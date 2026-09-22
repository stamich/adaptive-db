//! Current-state primary B+Tree backed by fixed pages and a shared buffer pool.

use std::{path::Path, sync::Arc};

use adb_buffer::{BufferPool, FilePageStore};
use adb_core::{Lsn, PageId, RowId, RowLocation};
use adb_page::PageKind;
use parking_lot::{Mutex, RwLock};

use crate::{
    codec::{decode_node, encode_node},
    error::BTreeError,
    meta::MetaStore,
    node::{InternalNode, LeafNode, Node, MAX_INTERNAL_KEYS, MAX_LEAF_ENTRIES},
};

/// Describes the separator promoted after a child page split.
struct Split {
    separator: RowId,
    right: PageId,
}

/// Persistent primary B+Tree mapping `RowId` to heap `RowLocation`.
pub struct BTree {
    pool: Arc<BufferPool>,
    root: RwLock<PageId>,
    meta: MetaStore,
    tree_lock: Mutex<()>,
}

/// Implements lookup, insertion, scanning, split propagation, and durable root publication.
impl BTree {
    /// Opens or creates a B+Tree and validates that the persisted root points inside the page file.
    pub fn open(
        data_path: impl AsRef<Path>,
        meta_path: impl AsRef<Path>,
        buffer_pages: usize,
    ) -> Result<Self, BTreeError> {
        let store = Arc::new(FilePageStore::open(data_path)?);
        let pool = Arc::new(BufferPool::new(store, buffer_pages));
        let meta = MetaStore::new(meta_path);
        let root = match meta.load()? {
            Some(root) => root,
            None => {
                let root = pool.allocate_page(PageKind::BTreeLeaf)?;
                pool.write(root, |page| {
                    encode_node(page, &Node::Leaf(LeafNode::empty()))
                })??;
                pool.flush_all()?;
                meta.save(root)?;
                root
            }
        };

        let page_count = pool.page_count()?;
        if root.0 >= page_count {
            return Err(BTreeError::Corrupt(format!(
                "root page is outside page file: root={}, pages={page_count}",
                root.0
            )));
        }

        Ok(Self {
            pool,
            root: RwLock::new(root),
            meta,
            tree_lock: Mutex::new(()),
        })
    }

    /// Looks up one row location while bounding traversal by the physical page count.
    pub fn get(&self, key: RowId) -> Result<Option<RowLocation>, BTreeError> {
        let _guard = self.tree_lock.lock();
        let mut page_id = *self.root.read();
        let max_steps = self.pool.page_count()?.saturating_add(1);
        let mut steps = 0u64;

        loop {
            steps = steps.saturating_add(1);
            if steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected while descending B+Tree".into(),
                ));
            }
            let node = self.pool.read(page_id, decode_node)??;
            match node {
                Node::Leaf(leaf) => {
                    return Ok(leaf
                        .keys
                        .binary_search(&key)
                        .ok()
                        .map(|index| leaf.values[index]));
                }
                Node::Internal(internal) => {
                    let mut index = 0;
                    while index < internal.keys.len() && key >= internal.keys[index] {
                        index += 1;
                    }
                    page_id = internal.children[index];
                }
            }
        }
    }

    /// Inserts or replaces one mapping without attaching a WAL LSN.
    pub fn insert(&self, key: RowId, value: RowLocation) -> Result<(), BTreeError> {
        self.insert_at_lsn(key, value, Lsn(0))
    }

    /// Inserts or replaces one mapping and stamps all modified pages with the supplied WAL LSN.
    pub fn insert_at_lsn(
        &self,
        key: RowId,
        value: RowLocation,
        lsn: Lsn,
    ) -> Result<(), BTreeError> {
        let _guard = self.tree_lock.lock();
        let root = *self.root.read();
        if let Some(split) = self.insert_recursive(root, key, value, lsn, 0)? {
            let new_root = self.pool.allocate_page(PageKind::BTreeInternal)?;
            let node = Node::Internal(InternalNode {
                keys: vec![split.separator],
                children: vec![root, split.right],
            });
            self.pool.write(new_root, |page| {
                page.set_page_lsn(lsn);
                encode_node(page, &node)
            })??;
            *self.root.write() = new_root;
            self.pool.flush_all()?;
            self.meta.save(new_root)?;
        }
        Ok(())
    }

    /// Flushes every dirty buffered page to the underlying page file.
    pub fn flush(&self) -> Result<(), BTreeError> {
        self.pool.flush_all()?;
        Ok(())
    }

    /// Returns the currently published root page id.
    pub fn root_page_id(&self) -> PageId {
        *self.root.read()
    }

    /// Returns the number of complete physical pages in the backing file.
    pub fn page_count(&self) -> Result<u64, BTreeError> {
        Ok(self.pool.page_count()?)
    }

    /// Scans all leaves in key order while detecting cycles or invalid leaf links.
    pub fn scan_all(&self) -> Result<Vec<(RowId, RowLocation)>, BTreeError> {
        let _guard = self.tree_lock.lock();
        let mut page_id = *self.root.read();
        let max_steps = self.pool.page_count()?.saturating_add(1);
        let mut descent_steps = 0u64;

        loop {
            descent_steps = descent_steps.saturating_add(1);
            if descent_steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected while locating first leaf".into(),
                ));
            }
            match self.pool.read(page_id, decode_node)?? {
                Node::Leaf(_) => break,
                Node::Internal(internal) => page_id = internal.children[0],
            }
        }

        let mut out = Vec::new();
        let mut leaf_steps = 0u64;
        loop {
            leaf_steps = leaf_steps.saturating_add(1);
            if leaf_steps > max_steps {
                return Err(BTreeError::Corrupt("cycle detected in leaf chain".into()));
            }
            let node = self.pool.read(page_id, decode_node)??;
            let Node::Leaf(leaf) = node else {
                return Err(BTreeError::Corrupt(
                    "leaf chain points to an internal node".into(),
                ));
            };
            out.extend(leaf.keys.iter().copied().zip(leaf.values.iter().copied()));
            match leaf.next {
                Some(next) => page_id = next,
                None => break,
            }
        }
        Ok(out)
    }

    /// Recursively inserts a mapping and returns a promoted split when the current page overflows.
    fn insert_recursive(
        &self,
        page_id: PageId,
        key: RowId,
        value: RowLocation,
        lsn: Lsn,
        depth: usize,
    ) -> Result<Option<Split>, BTreeError> {
        if depth > 1024 {
            return Err(BTreeError::Corrupt(
                "B+Tree depth exceeds hardened limit".into(),
            ));
        }
        let node = self.pool.read(page_id, decode_node)??;
        match node {
            Node::Leaf(mut leaf) => {
                match leaf.keys.binary_search(&key) {
                    Ok(index) => {
                        leaf.values[index] = value;
                        self.pool.write(page_id, |page| {
                            page.set_page_lsn(lsn);
                            encode_node(page, &Node::Leaf(leaf))
                        })??;
                        return Ok(None);
                    }
                    Err(index) => {
                        leaf.keys.insert(index, key);
                        leaf.values.insert(index, value);
                    }
                }

                if leaf.keys.len() <= MAX_LEAF_ENTRIES {
                    self.pool.write(page_id, |page| {
                        page.set_page_lsn(lsn);
                        encode_node(page, &Node::Leaf(leaf))
                    })??;
                    return Ok(None);
                }

                let split_at = leaf.keys.len() / 2;
                let right_keys = leaf.keys.split_off(split_at);
                let right_values = leaf.values.split_off(split_at);
                let right_page = self.pool.allocate_page(PageKind::BTreeLeaf)?;
                let old_next = leaf.next;
                leaf.next = Some(right_page);
                let right = LeafNode {
                    keys: right_keys,
                    values: right_values,
                    next: old_next,
                };
                let separator = right.keys[0];
                self.pool.write(page_id, |page| {
                    page.set_page_lsn(lsn);
                    encode_node(page, &Node::Leaf(leaf))
                })??;
                self.pool.write(right_page, |page| {
                    page.set_page_lsn(lsn);
                    encode_node(page, &Node::Leaf(right))
                })??;
                Ok(Some(Split {
                    separator,
                    right: right_page,
                }))
            }
            Node::Internal(mut internal) => {
                let mut child_index = 0;
                while child_index < internal.keys.len() && key >= internal.keys[child_index] {
                    child_index += 1;
                }
                let child = internal.children[child_index];
                if let Some(split) = self.insert_recursive(child, key, value, lsn, depth + 1)? {
                    internal.keys.insert(child_index, split.separator);
                    internal.children.insert(child_index + 1, split.right);
                } else {
                    return Ok(None);
                }

                if internal.keys.len() <= MAX_INTERNAL_KEYS {
                    self.pool.write(page_id, |page| {
                        page.set_page_lsn(lsn);
                        encode_node(page, &Node::Internal(internal))
                    })??;
                    return Ok(None);
                }

                let mid = internal.keys.len() / 2;
                let promoted = internal.keys[mid];
                let right_keys = internal.keys.split_off(mid + 1);
                internal.keys.truncate(mid);
                let right_children = internal.children.split_off(mid + 1);
                let right_page = self.pool.allocate_page(PageKind::BTreeInternal)?;
                let right = InternalNode {
                    keys: right_keys,
                    children: right_children,
                };
                self.pool.write(page_id, |page| {
                    page.set_page_lsn(lsn);
                    encode_node(page, &Node::Internal(internal))
                })??;
                self.pool.write(right_page, |page| {
                    page.set_page_lsn(lsn);
                    encode_node(page, &Node::Internal(right))
                })??;
                Ok(Some(Split {
                    separator: promoted,
                    right: right_page,
                }))
            }
        }
    }
}
