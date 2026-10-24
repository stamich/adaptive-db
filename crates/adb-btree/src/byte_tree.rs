//! Page-based B+Tree over variable-length byte keys and byte values (since 2.3.0).
//!
//! The primary and version indexes map fixed-width keys to heap locations ([`crate::BPlusTree`]).
//! Secondary indexes need composite keys of any length (order-preserving encodings, see
//! `adb_core::key_encoding`) and small payloads (end timestamp, covered columns), so this tree
//! stores both as byte strings and sizes its nodes by bytes instead of by entry count:
//!
//! ```text
//! leaf:     u16 count | 6 pad | u64 next (u64::MAX = none) | count x (u16 key_len | u16 value_len | key | value)
//! internal: u16 count | 6 pad | (count + 1) x u64 child | count x (u16 key_len | key)
//! ```
//!
//! A node splits when its encoding would exceed the page payload, at the entry that halves its
//! bytes. With keys of at most [`MAX_KEY_BYTES`] and values of at most [`MAX_VALUE_BYTES`] both
//! halves always fit, so every page holds at least four entries.
//!
//! The policies match the fixed-width tree: deletes never merge nodes (empty leaves are legal
//! and skipped), nothing reaches disk before a checkpoint collects [`ByteTree::journal_writes`],
//! and every descent and leaf walk is bounded so corruption yields [`BTreeError::Corrupt`].
//!
//! Range scans are *keyset* scans: [`ByteTree::range`] and [`ByteTree::range_rev`] return at most
//! `limit` entries, and a caller resumes after the last key it saw by passing it as an
//! exclusive bound. No cursor state is held between calls, so a scan never pins pages or locks.

use std::{
    ops::Bound,
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_buffer::{BufferPool, FilePageStore};
use adb_core::{Lsn, PageId};
use adb_journal::FileWrite;
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE, PAGE_SIZE};
use parking_lot::Mutex;

use crate::{error::BTreeError, meta::MetaStore};

/// Longest key accepted, in bytes.
pub const MAX_KEY_BYTES: usize = 1024;
/// Longest value accepted, in bytes.
pub const MAX_VALUE_BYTES: usize = 2048;
/// Hard limit on tree height; far above any real tree, low enough to stop corrupt cycles.
const MAX_DEPTH: usize = 64;
/// Bytes available to a node after the common page header.
const PAYLOAD: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
/// Leaf header: entry count (u16), padding, right-sibling page id (u64).
const LEAF_HEADER: usize = 16;
/// Internal-node header: key count (u16) plus padding.
const INTERNAL_HEADER: usize = 8;
/// Sentinel stored in `next` when a leaf has no right sibling.
const NONE_PAGE: u64 = u64::MAX;

/// An owned key or value.
type Bytes = Vec<u8>;

/// Decoded leaf: ordered entries and the right-sibling link.
#[derive(Debug, Clone, Default)]
struct Leaf {
    /// `(key, value)` in strictly increasing key order.
    entries: Vec<(Bytes, Bytes)>,
    /// Next leaf in key order.
    next: Option<PageId>,
}

/// Decoded internal node: child `i` holds keys `< keys[i]`, child `i + 1` keys `>= keys[i]`.
#[derive(Debug, Clone)]
struct Internal {
    /// Separator keys.
    keys: Vec<Bytes>,
    /// Child pages (`keys.len() + 1`).
    children: Vec<PageId>,
}

/// A decoded node.
#[derive(Debug, Clone)]
enum Node {
    /// Leaf node.
    Leaf(Leaf),
    /// Internal node.
    Internal(Internal),
}

/// Separator and new right sibling produced by a split.
struct Split {
    /// Smallest key of the new right node; inserted into the parent.
    separator: Bytes,
    /// Page of the new right node.
    right: PageId,
}

/// Shape counters of a tree (input of index statistics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeShape {
    /// Levels from the root to the leaves (1 for a lone root leaf).
    pub height: u32,
    /// Leaf pages reachable from the root.
    pub leaf_pages: u64,
    /// Internal pages reachable from the root.
    pub internal_pages: u64,
    /// Entries in all leaves.
    pub entries: u64,
}

/// Persistent B+Tree mapping byte keys to byte values.
pub struct ByteTree {
    /// Page cache of the tree file.
    pool: Arc<BufferPool>,
    /// Path of the tree file (target of journal writes).
    data_path: PathBuf,
    /// Root-pointer file.
    meta: MetaStore,
    /// Current root; guarded together with every structural operation.
    root: Mutex<PageId>,
}

impl ByteTree {
    /// Opens an existing tree or creates an empty one.
    pub fn open(
        data_path: impl AsRef<Path>,
        meta_path: impl AsRef<Path>,
        buffer_pages: usize,
    ) -> Result<Self, BTreeError> {
        let data_path = data_path.as_ref().to_path_buf();
        let store = Arc::new(FilePageStore::open(&data_path)?);
        let pool = Arc::new(BufferPool::new(store, buffer_pages)?);
        let meta = MetaStore::new(meta_path);
        let root = match meta.load()? {
            Some(root) => {
                if root.0 >= pool.page_count() {
                    return Err(BTreeError::Corrupt(format!(
                        "root page {} is outside the page file ({} pages)",
                        root.0,
                        pool.page_count()
                    )));
                }
                root
            }
            None => {
                // A brand-new tree: nothing references it yet, so a direct flush is safe.
                let root = pool.allocate_page(PageKind::ByteTreeLeaf)?;
                pool.write(root, |page| encode(page, &Node::Leaf(Leaf::default())))??;
                pool.flush_all()?;
                meta.save(root)?;
                root
            }
        };
        Ok(Self {
            pool,
            data_path,
            meta,
            root: Mutex::new(root),
        })
    }

    /// Value stored under `key`.
    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>, BTreeError> {
        let root = self.root.lock();
        let (_, leaf) = self.leaf_for(*root, Some(key))?;
        Ok(leaf
            .entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
            .ok()
            .map(|index| leaf.entries[index].1.clone()))
    }

    /// Inserts or replaces the value of `key`, stamping modified pages with `lsn`.
    pub fn insert(&self, key: &[u8], value: &[u8], lsn: Lsn) -> Result<(), BTreeError> {
        check_sizes(key, value)?;
        let mut root = self.root.lock();
        if let Some(split) = self.insert_into(*root, key, value, lsn, 0)? {
            let new_root = self.pool.allocate_page(PageKind::ByteTreeInternal)?;
            self.write_node(
                new_root,
                lsn,
                &Node::Internal(Internal {
                    keys: vec![split.separator],
                    children: vec![*root, split.right],
                }),
            )?;
            *root = new_root;
        }
        Ok(())
    }

    /// Removes `key` and returns its value. Nodes are never merged.
    pub fn remove(&self, key: &[u8], lsn: Lsn) -> Result<Option<Bytes>, BTreeError> {
        let root = self.root.lock();
        let (page_id, mut leaf) = self.leaf_for(*root, Some(key))?;
        let Ok(index) = leaf
            .entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        else {
            return Ok(None);
        };
        let (_, value) = leaf.entries.remove(index);
        self.write_node(page_id, lsn, &Node::Leaf(leaf))?;
        Ok(Some(value))
    }

    /// Up to `limit` entries within `(lower, upper)` in ascending key order.
    pub fn range(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Bytes, Bytes)>, BTreeError> {
        let root = self.root.lock();
        let seek = match lower {
            Bound::Included(key) | Bound::Excluded(key) => Some(key),
            Bound::Unbounded => None,
        };
        let (_, mut leaf) = self.leaf_for(*root, seek)?;
        let mut out = Vec::new();
        let mut steps = 0u64;
        loop {
            for (key, value) in leaf.entries {
                if !after_lower(&key, lower) {
                    continue;
                }
                if !before_upper(&key, upper) || out.len() >= limit {
                    return Ok(out);
                }
                out.push((key, value));
            }
            let Some(next) = leaf.next else {
                return Ok(out);
            };
            steps += 1;
            if steps > self.pool.page_count() {
                return Err(BTreeError::Corrupt("cycle in leaf chain".into()));
            }
            leaf = self.read_leaf(next)?;
        }
    }

    /// Up to `limit` entries within `(lower, upper)` in descending key order.
    ///
    /// Leaves link only to their right sibling, so each step descends again to the leaf holding
    /// the greatest key below the current position: `O(log n)` per visited leaf.
    pub fn range_rev(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Bytes, Bytes)>, BTreeError> {
        let root = self.root.lock();
        let mut out: Vec<(Bytes, Bytes)> = Vec::new();
        let mut bound: Bound<Bytes> = match upper {
            Bound::Included(key) => Bound::Included(key.to_vec()),
            Bound::Excluded(key) => Bound::Excluded(key.to_vec()),
            Bound::Unbounded => Bound::Unbounded,
        };
        for _ in 0..=self.pool.page_count() {
            let Some(found) = self.last_below(*root, as_ref(&bound), 0)? else {
                return Ok(out);
            };
            for (key, value) in found.into_iter().rev() {
                if !after_lower(&key, lower) || out.len() >= limit {
                    return Ok(out);
                }
                out.push((key, value));
            }
            match out.last() {
                Some((key, _)) => bound = Bound::Excluded(key.clone()),
                None => return Ok(out),
            }
        }
        Err(BTreeError::Corrupt(
            "reverse scan visited more leaves than pages".into(),
        ))
    }

    /// Height, page counts and entries of the tree (walks every reachable page).
    pub fn shape(&self) -> Result<TreeShape, BTreeError> {
        let root = self.root.lock();
        let mut shape = TreeShape::default();
        let mut level = vec![*root];
        for depth in 0..MAX_DEPTH {
            shape.height = depth as u32 + 1;
            let mut next = Vec::new();
            for page in level {
                match self.read_node(page)? {
                    Node::Leaf(leaf) => {
                        shape.leaf_pages += 1;
                        shape.entries += leaf.entries.len() as u64;
                    }
                    Node::Internal(internal) => {
                        shape.internal_pages += 1;
                        next.extend(internal.children);
                    }
                }
                if shape.leaf_pages + shape.internal_pages > self.pool.page_count() {
                    return Err(BTreeError::Corrupt(
                        "tree reaches more nodes than pages".into(),
                    ));
                }
            }
            if next.is_empty() {
                return Ok(shape);
            }
            level = next;
        }
        Err(BTreeError::Corrupt("tree depth exceeds limit".into()))
    }

    /// Current root page.
    pub fn root_page_id(&self) -> PageId {
        *self.root.lock()
    }

    /// Logical number of pages in the tree file.
    pub fn page_count(&self) -> u64 {
        self.pool.page_count()
    }

    /// Pages changed since the last checkpoint.
    pub fn dirty_page_count(&self) -> usize {
        self.pool.dirty_count()
    }

    /// Dirty pages plus the root pointer, for the checkpoint journal.
    pub fn journal_writes(&self) -> Result<Vec<FileWrite>, BTreeError> {
        let root = self.root.lock();
        let mut writes = self.pool.journal_writes(&self.data_path)?;
        writes.push(self.meta.journal_write(*root)?);
        Ok(writes)
    }

    /// Marks the state captured by [`journal_writes`](Self::journal_writes) as persisted.
    pub fn mark_clean(&self) {
        self.pool.mark_clean();
    }

    /// Persists all pages and the root directly (not crash-atomic; for standalone use).
    pub fn flush(&self) -> Result<(), BTreeError> {
        let root = self.root.lock();
        self.pool.flush_all()?;
        self.meta.save(*root)
    }

    // ----- internals -------------------------------------------------------------------------

    /// Decodes the node stored in `page_id`.
    fn read_node(&self, page_id: PageId) -> Result<Node, BTreeError> {
        self.pool.read(page_id, decode)?
    }

    /// Decodes `page_id`, which must be a leaf (used when following the leaf chain).
    fn read_leaf(&self, page_id: PageId) -> Result<Leaf, BTreeError> {
        match self.read_node(page_id)? {
            Node::Leaf(leaf) => Ok(leaf),
            Node::Internal(_) => Err(BTreeError::Corrupt(
                "leaf chain points to an internal node".into(),
            )),
        }
    }

    /// Encodes `node` into `page_id` and stamps the page with `lsn`.
    fn write_node(&self, page_id: PageId, lsn: Lsn, node: &Node) -> Result<(), BTreeError> {
        self.pool.write(page_id, |page| {
            page.set_page_lsn(lsn);
            encode(page, node)
        })?
    }

    /// Descends to the leaf that may contain `key` (`None` = leftmost leaf).
    fn leaf_for(&self, root: PageId, key: Option<&[u8]>) -> Result<(PageId, Leaf), BTreeError> {
        let mut page_id = root;
        for _ in 0..MAX_DEPTH {
            match self.read_node(page_id)? {
                Node::Leaf(leaf) => return Ok((page_id, leaf)),
                Node::Internal(internal) => {
                    let child = key.map_or(0, |key| route(&internal.keys, key));
                    page_id = internal.children[child];
                }
            }
        }
        Err(BTreeError::Corrupt("tree depth exceeds limit".into()))
    }

    /// The entries `< bound` (or `<=` for an inclusive bound) of the leaf holding the greatest
    /// such key, in ascending order; skips empty leaves. `None` if no key is below the bound.
    fn last_below(
        &self,
        page_id: PageId,
        bound: Bound<&[u8]>,
        depth: usize,
    ) -> Result<Option<Vec<(Bytes, Bytes)>>, BTreeError> {
        if depth > MAX_DEPTH {
            return Err(BTreeError::Corrupt("tree depth exceeds limit".into()));
        }
        match self.read_node(page_id)? {
            Node::Leaf(leaf) => {
                let kept: Vec<(Bytes, Bytes)> = leaf
                    .entries
                    .into_iter()
                    .filter(|(key, _)| before_upper(key, bound))
                    .collect();
                Ok((!kept.is_empty()).then_some(kept))
            }
            Node::Internal(internal) => {
                let first = match bound {
                    Bound::Included(key) | Bound::Excluded(key) => route(&internal.keys, key),
                    Bound::Unbounded => internal.children.len() - 1,
                };
                for child in internal.children[..=first].iter().rev() {
                    if let Some(found) = self.last_below(*child, bound, depth + 1)? {
                        return Ok(Some(found));
                    }
                }
                Ok(None)
            }
        }
    }

    /// Inserts into the subtree rooted at `page_id`; returns the split to propagate, if any.
    fn insert_into(
        &self,
        page_id: PageId,
        key: &[u8],
        value: &[u8],
        lsn: Lsn,
        depth: usize,
    ) -> Result<Option<Split>, BTreeError> {
        if depth > MAX_DEPTH {
            return Err(BTreeError::Corrupt("tree depth exceeds limit".into()));
        }
        match self.read_node(page_id)? {
            Node::Leaf(mut leaf) => {
                match leaf
                    .entries
                    .binary_search_by(|(k, _)| k.as_slice().cmp(key))
                {
                    Ok(index) => leaf.entries[index].1 = value.to_vec(),
                    Err(index) => leaf.entries.insert(index, (key.to_vec(), value.to_vec())),
                }
                if leaf_bytes(&leaf.entries) <= PAYLOAD {
                    self.write_node(page_id, lsn, &Node::Leaf(leaf))?;
                    return Ok(None);
                }
                let at = split_point(leaf.entries.iter().map(|(k, v)| 4 + k.len() + v.len()));
                let right_page = self.pool.allocate_page(PageKind::ByteTreeLeaf)?;
                let right = Leaf {
                    entries: leaf.entries.split_off(at),
                    next: leaf.next.replace(right_page),
                };
                let separator = right.entries[0].0.clone();
                self.write_node(page_id, lsn, &Node::Leaf(leaf))?;
                self.write_node(right_page, lsn, &Node::Leaf(right))?;
                Ok(Some(Split {
                    separator,
                    right: right_page,
                }))
            }
            Node::Internal(mut internal) => {
                let child = route(&internal.keys, key);
                let Some(split) =
                    self.insert_into(internal.children[child], key, value, lsn, depth + 1)?
                else {
                    return Ok(None);
                };
                internal.keys.insert(child, split.separator);
                internal.children.insert(child + 1, split.right);
                if internal_bytes(&internal) <= PAYLOAD {
                    self.write_node(page_id, lsn, &Node::Internal(internal))?;
                    return Ok(None);
                }
                let mid = split_point(internal.keys.iter().map(|k| 10 + k.len()));
                let promoted = internal.keys[mid].clone();
                let right = Internal {
                    keys: internal.keys.split_off(mid + 1),
                    children: internal.children.split_off(mid + 1),
                };
                internal.keys.truncate(mid);
                let right_page = self.pool.allocate_page(PageKind::ByteTreeInternal)?;
                self.write_node(page_id, lsn, &Node::Internal(internal))?;
                self.write_node(right_page, lsn, &Node::Internal(right))?;
                Ok(Some(Split {
                    separator: promoted,
                    right: right_page,
                }))
            }
        }
    }
}

/// Index of the child of an internal node that may contain `key`.
fn route(keys: &[Bytes], key: &[u8]) -> usize {
    keys.partition_point(|separator| separator.as_slice() <= key)
}

/// Whether `key` lies after the lower bound.
fn after_lower(key: &[u8], lower: Bound<&[u8]>) -> bool {
    match lower {
        Bound::Included(bound) => key >= bound,
        Bound::Excluded(bound) => key > bound,
        Bound::Unbounded => true,
    }
}

/// Whether `key` lies before the upper bound.
fn before_upper(key: &[u8], upper: Bound<&[u8]>) -> bool {
    match upper {
        Bound::Included(bound) => key <= bound,
        Bound::Excluded(bound) => key < bound,
        Bound::Unbounded => true,
    }
}

/// Borrows an owned bound.
fn as_ref(bound: &Bound<Bytes>) -> Bound<&[u8]> {
    match bound {
        Bound::Included(key) => Bound::Included(key),
        Bound::Excluded(key) => Bound::Excluded(key),
        Bound::Unbounded => Bound::Unbounded,
    }
}

/// Rejects keys and values beyond the limits that keep every node splittable.
fn check_sizes(key: &[u8], value: &[u8]) -> Result<(), BTreeError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(BTreeError::Limit(format!(
            "key of {} bytes; accepted size is 1..={MAX_KEY_BYTES}",
            key.len()
        )));
    }
    if value.len() > MAX_VALUE_BYTES {
        return Err(BTreeError::Limit(format!(
            "value of {} bytes exceeds {MAX_VALUE_BYTES}",
            value.len()
        )));
    }
    Ok(())
}

/// Index at which the cumulative size of `sizes` first reaches half of the total (at least 1,
/// so both halves are non-empty).
fn split_point(sizes: impl Iterator<Item = usize> + Clone) -> usize {
    let total: usize = sizes.clone().sum();
    let count = sizes.clone().count();
    let mut running = 0;
    for (index, size) in sizes.enumerate() {
        running += size;
        if running * 2 >= total {
            return (index + 1).clamp(1, count - 1);
        }
    }
    count / 2
}

/// Encoded size of a leaf with `entries`.
fn leaf_bytes(entries: &[(Bytes, Bytes)]) -> usize {
    LEAF_HEADER
        + entries
            .iter()
            .map(|(k, v)| 4 + k.len() + v.len())
            .sum::<usize>()
}

/// Encoded size of an internal node.
fn internal_bytes(node: &Internal) -> usize {
    INTERNAL_HEADER + node.children.len() * 8 + node.keys.iter().map(|k| 2 + k.len()).sum::<usize>()
}

/// Validates and encodes one node into `page`.
fn encode(page: &mut Page, node: &Node) -> Result<(), BTreeError> {
    let (kind, size) = match node {
        Node::Leaf(leaf) => {
            ensure_sorted(leaf.entries.iter().map(|(k, _)| k.as_slice()))?;
            (PageKind::ByteTreeLeaf, leaf_bytes(&leaf.entries))
        }
        Node::Internal(internal) => {
            if internal.children.len() != internal.keys.len() + 1 {
                return Err(BTreeError::Corrupt("invalid internal shape".into()));
            }
            ensure_sorted(internal.keys.iter().map(Vec::as_slice))?;
            (PageKind::ByteTreeInternal, internal_bytes(internal))
        }
    };
    if size > PAYLOAD {
        return Err(BTreeError::Corrupt(format!(
            "node of {size} bytes exceeds a page"
        )));
    }
    page.set_kind(kind);
    let data = &mut page.bytes_mut()[PAGE_HEADER_SIZE..];
    data.fill(0);
    let mut writer = Writer { data, at: 0 };
    match node {
        Node::Leaf(leaf) => {
            writer.put(&(leaf.entries.len() as u16).to_le_bytes());
            writer.at = 8;
            writer.put(&leaf.next.map_or(NONE_PAGE, |p| p.0).to_le_bytes());
            for (key, value) in &leaf.entries {
                writer.put(&(key.len() as u16).to_le_bytes());
                writer.put(&(value.len() as u16).to_le_bytes());
                writer.put(key);
                writer.put(value);
            }
        }
        Node::Internal(internal) => {
            writer.put(&(internal.keys.len() as u16).to_le_bytes());
            writer.at = INTERNAL_HEADER;
            for child in &internal.children {
                writer.put(&child.0.to_le_bytes());
            }
            for key in &internal.keys {
                writer.put(&(key.len() as u16).to_le_bytes());
                writer.put(key);
            }
        }
    }
    Ok(())
}

/// Decodes and validates one node page; every read is bounds-checked.
fn decode(page: &Page) -> Result<Node, BTreeError> {
    let data = &page.bytes()[PAGE_HEADER_SIZE..];
    let mut reader = Reader { data, at: 0 };
    match page
        .kind()
        .map_err(|e| BTreeError::Corrupt(e.to_string()))?
    {
        PageKind::ByteTreeLeaf => {
            let count = reader.u16()? as usize;
            reader.at = 8;
            let next = reader.u64()?;
            let mut entries = Vec::with_capacity(count.min(PAYLOAD / 4));
            for _ in 0..count {
                let key_len = reader.u16()? as usize;
                let value_len = reader.u16()? as usize;
                if key_len == 0 || key_len > MAX_KEY_BYTES || value_len > MAX_VALUE_BYTES {
                    return Err(BTreeError::Corrupt("invalid entry length".into()));
                }
                let key = reader.bytes(key_len)?.to_vec();
                let value = reader.bytes(value_len)?.to_vec();
                entries.push((key, value));
            }
            ensure_sorted(entries.iter().map(|(k, _)| k.as_slice()))?;
            Ok(Node::Leaf(Leaf {
                entries,
                next: (next != NONE_PAGE).then_some(PageId(next)),
            }))
        }
        PageKind::ByteTreeInternal => {
            let count = reader.u16()? as usize;
            if INTERNAL_HEADER + (count + 1) * 8 > PAYLOAD {
                return Err(BTreeError::Corrupt("invalid internal count".into()));
            }
            reader.at = INTERNAL_HEADER;
            let mut children = Vec::with_capacity(count + 1);
            for _ in 0..=count {
                children.push(PageId(reader.u64()?));
            }
            let mut keys = Vec::with_capacity(count);
            for _ in 0..count {
                let len = reader.u16()? as usize;
                if len == 0 || len > MAX_KEY_BYTES {
                    return Err(BTreeError::Corrupt("invalid separator length".into()));
                }
                keys.push(reader.bytes(len)?.to_vec());
            }
            ensure_sorted(keys.iter().map(Vec::as_slice))?;
            Ok(Node::Internal(Internal { keys, children }))
        }
        other => Err(BTreeError::Corrupt(format!(
            "unexpected page kind {other:?} in a byte tree"
        ))),
    }
}

/// Requires strictly increasing keys, which binary search and routing rely on.
fn ensure_sorted<'a>(keys: impl Iterator<Item = &'a [u8]>) -> Result<(), BTreeError> {
    let mut previous: Option<&[u8]> = None;
    for key in keys {
        if previous.is_some_and(|previous| previous >= key) {
            return Err(BTreeError::Corrupt("node keys not strictly sorted".into()));
        }
        previous = Some(key);
    }
    Ok(())
}

/// Sequential writer over a node payload (sizes were checked by the caller).
struct Writer<'a> {
    /// Node payload.
    data: &'a mut [u8],
    /// Write position.
    at: usize,
}

impl Writer<'_> {
    /// Copies `bytes` at the current position and advances.
    fn put(&mut self, bytes: &[u8]) {
        self.data[self.at..self.at + bytes.len()].copy_from_slice(bytes);
        self.at += bytes.len();
    }
}

/// Sequential, bounds-checked reader over a node payload.
struct Reader<'a> {
    /// Node payload.
    data: &'a [u8],
    /// Read position.
    at: usize,
}

impl<'a> Reader<'a> {
    /// The next `len` bytes.
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], BTreeError> {
        let slice = self
            .data
            .get(self.at..self.at + len)
            .ok_or_else(|| BTreeError::Corrupt("read outside node payload".into()))?;
        self.at += len;
        Ok(slice)
    }

    /// The next little-endian `u16`.
    fn u16(&mut self) -> Result<u16, BTreeError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// The next little-endian `u64`.
    fn u64(&mut self) -> Result<u64, BTreeError> {
        let mut array = [0u8; 8];
        array.copy_from_slice(self.bytes(8)?);
        Ok(u64::from_le_bytes(array))
    }
}
