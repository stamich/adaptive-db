//! Generic page-based B+Tree.
//!
//! * Lookups, range scans and floor lookups descend from the root; range scans then follow
//!   the leaf chain.
//! * Inserts split full nodes bottom-up; a root split allocates a new root in memory.
//! * Removes delete the entry from its leaf without rebalancing. Underfull or empty leaves are
//!   legal and skipped by every read path (the same lazy policy PostgreSQL uses).
//! * Nothing is written to disk by mutations (no-steal buffer pool). The checkpoint collects
//!   dirty pages and the root pointer via [`BPlusTree::journal_writes`].
//!
//! Every descent is bounded by a depth limit, and leaf-chain walks by the page count, so a
//! corrupted file yields [`BTreeError::Corrupt`] instead of looping forever.

use std::{
    marker::PhantomData,
    ops::Bound,
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_buffer::{BufferPool, FilePageStore};
use adb_core::{Lsn, PageId, RowLocation};
use adb_journal::FileWrite;
use adb_page::PageKind;
use parking_lot::Mutex;

use crate::{
    codec::{decode_node, encode_node},
    error::BTreeError,
    key::TreeKey,
    meta::MetaStore,
    node::{InternalNode, LeafNode, Node},
};

/// Hard limit on tree height; far above any real tree, low enough to stop corrupt cycles.
const MAX_DEPTH: usize = 64;

/// Separator and new right sibling produced by a split.
struct Split<K> {
    separator: K,
    right: PageId,
}

/// Persistent B+Tree mapping `K` to heap [`RowLocation`]s.
pub struct BPlusTree<K: TreeKey> {
    pool: Arc<BufferPool>,
    data_path: PathBuf,
    meta: MetaStore,
    /// Current root; guarded together with every structural operation.
    root: Mutex<PageId>,
    _key: PhantomData<K>,
}

impl<K: TreeKey> BPlusTree<K> {
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
                let root = pool.allocate_page(PageKind::BTreeLeaf)?;
                pool.write(root, |page| {
                    encode_node(page, &Node::<K>::Leaf(LeafNode::empty()))
                })??;
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
            _key: PhantomData,
        })
    }

    /// Exact-match lookup.
    pub fn get(&self, key: K) -> Result<Option<RowLocation>, BTreeError> {
        let root = self.root.lock();
        let leaf = self.leaf_for(*root, Some(&key))?.1;
        Ok(leaf
            .keys
            .binary_search(&key)
            .ok()
            .map(|index| leaf.values[index]))
    }

    /// Greatest entry whose key is `<= key`.
    pub fn get_floor(&self, key: K) -> Result<Option<(K, RowLocation)>, BTreeError> {
        let root = self.root.lock();
        self.floor_in(*root, &key, 0)
    }

    /// Entries within `(start, end)` bounds in key order, at most `limit` of them.
    pub fn scan(
        &self,
        start: Bound<K>,
        end: Bound<K>,
        limit: usize,
    ) -> Result<Vec<(K, RowLocation)>, BTreeError> {
        let root = self.root.lock();
        let seek = match &start {
            Bound::Included(key) | Bound::Excluded(key) => Some(key),
            Bound::Unbounded => None,
        };
        let mut leaf = self.leaf_for(*root, seek)?.1;
        let mut out = Vec::new();
        let mut steps = 0u64;
        loop {
            for (key, value) in leaf.keys.iter().zip(&leaf.values) {
                let after_start = match &start {
                    Bound::Included(s) => key >= s,
                    Bound::Excluded(s) => key > s,
                    Bound::Unbounded => true,
                };
                if !after_start {
                    continue;
                }
                let before_end = match &end {
                    Bound::Included(e) => key <= e,
                    Bound::Excluded(e) => key < e,
                    Bound::Unbounded => true,
                };
                if !before_end || out.len() >= limit {
                    return Ok(out);
                }
                out.push((*key, *value));
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

    /// Every entry in key order.
    pub fn scan_all(&self) -> Result<Vec<(K, RowLocation)>, BTreeError> {
        self.scan(Bound::Unbounded, Bound::Unbounded, usize::MAX)
    }

    /// Inserts or replaces a mapping.
    pub fn insert(&self, key: K, value: RowLocation) -> Result<(), BTreeError> {
        self.insert_at_lsn(key, value, Lsn(0))
    }

    /// Inserts or replaces a mapping, stamping modified pages with the WAL position `lsn`.
    pub fn insert_at_lsn(&self, key: K, value: RowLocation, lsn: Lsn) -> Result<(), BTreeError> {
        let mut root = self.root.lock();
        if let Some(split) = self.insert_into(*root, key, value, lsn, 0)? {
            let new_root = self.pool.allocate_page(PageKind::BTreeInternal)?;
            self.write_node(
                new_root,
                lsn,
                &Node::Internal(InternalNode {
                    keys: vec![split.separator],
                    children: vec![*root, split.right],
                }),
            )?;
            *root = new_root;
        }
        Ok(())
    }

    /// Removes a mapping and returns its value. Nodes are never merged.
    pub fn remove(&self, key: K, lsn: Lsn) -> Result<Option<RowLocation>, BTreeError> {
        let root = self.root.lock();
        let (page_id, mut leaf) = self.leaf_for(*root, Some(&key))?;
        let Ok(index) = leaf.keys.binary_search(&key) else {
            return Ok(None);
        };
        leaf.keys.remove(index);
        let value = leaf.values.remove(index);
        self.write_node(page_id, lsn, &Node::Leaf(leaf))?;
        Ok(Some(value))
    }

    /// Current root page.
    pub fn root_page_id(&self) -> PageId {
        *self.root.lock()
    }

    /// Logical number of pages in the index file.
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

    fn read_node(&self, page_id: PageId) -> Result<Node<K>, BTreeError> {
        self.pool.read(page_id, decode_node::<K>)?
    }

    fn read_leaf(&self, page_id: PageId) -> Result<LeafNode<K>, BTreeError> {
        match self.read_node(page_id)? {
            Node::Leaf(leaf) => Ok(leaf),
            Node::Internal(_) => Err(BTreeError::Corrupt(
                "leaf chain points to an internal node".into(),
            )),
        }
    }

    fn write_node(&self, page_id: PageId, lsn: Lsn, node: &Node<K>) -> Result<(), BTreeError> {
        self.pool.write(page_id, |page| {
            page.set_page_lsn(lsn);
            encode_node(page, node)
        })?
    }

    /// Descends to the leaf that may contain `key` (`None` = leftmost leaf).
    fn leaf_for(&self, root: PageId, key: Option<&K>) -> Result<(PageId, LeafNode<K>), BTreeError> {
        let mut page_id = root;
        for _ in 0..MAX_DEPTH {
            match self.read_node(page_id)? {
                Node::Leaf(leaf) => return Ok((page_id, leaf)),
                Node::Internal(internal) => {
                    let child = key.map_or(0, |key| internal.route(key));
                    page_id = internal.children[child];
                }
            }
        }
        Err(BTreeError::Corrupt("tree depth exceeds limit".into()))
    }

    fn floor_in(
        &self,
        page_id: PageId,
        key: &K,
        depth: usize,
    ) -> Result<Option<(K, RowLocation)>, BTreeError> {
        if depth > MAX_DEPTH {
            return Err(BTreeError::Corrupt("tree depth exceeds limit".into()));
        }
        match self.read_node(page_id)? {
            Node::Leaf(leaf) => {
                let index = leaf.keys.partition_point(|candidate| candidate <= key);
                Ok(index.checked_sub(1).map(|i| (leaf.keys[i], leaf.values[i])))
            }
            Node::Internal(internal) => {
                let route = internal.route(key);
                if let Some(found) = self.floor_in(internal.children[route], key, depth + 1)? {
                    return Ok(Some(found));
                }
                // Everything left of the routed child is < key; take the greatest entry there.
                for child in internal.children[..route].iter().rev() {
                    if let Some(found) = self.last_in(*child, depth + 1)? {
                        return Ok(Some(found));
                    }
                }
                Ok(None)
            }
        }
    }

    fn last_in(
        &self,
        page_id: PageId,
        depth: usize,
    ) -> Result<Option<(K, RowLocation)>, BTreeError> {
        if depth > MAX_DEPTH {
            return Err(BTreeError::Corrupt("tree depth exceeds limit".into()));
        }
        match self.read_node(page_id)? {
            Node::Leaf(leaf) => Ok(leaf.keys.last().copied().zip(leaf.values.last().copied())),
            Node::Internal(internal) => {
                for child in internal.children.iter().rev() {
                    if let Some(found) = self.last_in(*child, depth + 1)? {
                        return Ok(Some(found));
                    }
                }
                Ok(None)
            }
        }
    }

    fn insert_into(
        &self,
        page_id: PageId,
        key: K,
        value: RowLocation,
        lsn: Lsn,
        depth: usize,
    ) -> Result<Option<Split<K>>, BTreeError> {
        if depth > MAX_DEPTH {
            return Err(BTreeError::Corrupt("tree depth exceeds limit".into()));
        }
        match self.read_node(page_id)? {
            Node::Leaf(mut leaf) => {
                match leaf.keys.binary_search(&key) {
                    Ok(index) => leaf.values[index] = value,
                    Err(index) => {
                        leaf.keys.insert(index, key);
                        leaf.values.insert(index, value);
                    }
                }
                if leaf.keys.len() <= K::MAX_LEAF_ENTRIES {
                    self.write_node(page_id, lsn, &Node::Leaf(leaf))?;
                    return Ok(None);
                }
                let at = leaf.keys.len() / 2;
                let right_page = self.pool.allocate_page(PageKind::BTreeLeaf)?;
                let right = LeafNode {
                    keys: leaf.keys.split_off(at),
                    values: leaf.values.split_off(at),
                    next: leaf.next.replace(right_page),
                };
                let separator = right.keys[0];
                self.write_node(page_id, lsn, &Node::Leaf(leaf))?;
                self.write_node(right_page, lsn, &Node::Leaf(right))?;
                Ok(Some(Split {
                    separator,
                    right: right_page,
                }))
            }
            Node::Internal(mut internal) => {
                let route = internal.route(&key);
                let Some(split) =
                    self.insert_into(internal.children[route], key, value, lsn, depth + 1)?
                else {
                    return Ok(None);
                };
                internal.keys.insert(route, split.separator);
                internal.children.insert(route + 1, split.right);
                if internal.keys.len() <= K::MAX_INTERNAL_KEYS {
                    self.write_node(page_id, lsn, &Node::Internal(internal))?;
                    return Ok(None);
                }
                let mid = internal.keys.len() / 2;
                let promoted = internal.keys[mid];
                let right = InternalNode {
                    keys: internal.keys.split_off(mid + 1),
                    children: internal.children.split_off(mid + 1),
                };
                internal.keys.truncate(mid);
                let right_page = self.pool.allocate_page(PageKind::BTreeInternal)?;
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
