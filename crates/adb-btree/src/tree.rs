//! Module `tree` for crate `adb-btree`.
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

/// Represents `Split` state used by this subsystem.
struct Split {
    separator: RowId,
    right: PageId,
}

/// Represents `BTree` state used by this subsystem.
pub struct BTree {
    pool: Arc<BufferPool>,
    root: RwLock<PageId>,
    meta: MetaStore,
    tree_lock: Mutex<()>,
}

/// Implements behavior for `BTree`.
impl BTree {
    /// Implements the `open` operation used by this subsystem.
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

    /// Implements the `get` operation used by this subsystem.
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
                    return Ok(leaf.keys.binary_search(&key).ok().map(|i| leaf.values[i]))
                }
                Node::Internal(internal) => {
                    let mut idx = 0;
                    while idx < internal.keys.len() && key >= internal.keys[idx] {
                        idx += 1;
                    }
                    page_id = internal.children[idx];
                }
            }
        }
    }

    /// Implements the `insert` operation used by this subsystem.
    pub fn insert(&self, key: RowId, value: RowLocation) -> Result<(), BTreeError> {
        self.insert_at_lsn(key, value, Lsn(0))
    }

    /// Implements the `insert_at_lsn` operation used by this subsystem.
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

    /// Implements the `flush` operation used by this subsystem.
    pub fn flush(&self) -> Result<(), BTreeError> {
        self.pool.flush_all()?;
        Ok(())
    }

    /// Implements the `root_page_id` operation used by this subsystem.
    pub fn root_page_id(&self) -> PageId {
        *self.root.read()
    }

    /// Implements the `page_count` operation used by this subsystem.
    pub fn page_count(&self) -> Result<u64, BTreeError> {
        Ok(self.pool.page_count()?)
    }

    /// Implements the `scan_all` operation used by this subsystem.
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
                Node::Internal(internal) => {
                    page_id = internal.children[0];
                }
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
                    "leaf chain points to an internal node".to_string(),
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

    /// Implements the `insert_recursive` operation used by this subsystem.
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
                    Ok(i) => {
                        leaf.values[i] = value;
                        self.pool.write(page_id, |p| {
                            p.set_page_lsn(lsn);
                            encode_node(p, &Node::Leaf(leaf))
                        })??;
                        return Ok(None);
                    }
                    Err(i) => {
                        leaf.keys.insert(i, key);
                        leaf.values.insert(i, value);
                    }
                }
                if leaf.keys.len() <= MAX_LEAF_ENTRIES {
                    self.pool.write(page_id, |p| {
                        p.set_page_lsn(lsn);
                        encode_node(p, &Node::Leaf(leaf))
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
                self.pool.write(page_id, |p| {
                    p.set_page_lsn(lsn);
                    encode_node(p, &Node::Leaf(leaf))
                })??;
                self.pool.write(right_page, |p| {
                    p.set_page_lsn(lsn);
                    encode_node(p, &Node::Leaf(right))
                })??;
                Ok(Some(Split {
                    separator,
                    right: right_page,
                }))
            }
            Node::Internal(mut internal) => {
                let mut child_idx = 0;
                while child_idx < internal.keys.len() && key >= internal.keys[child_idx] {
                    child_idx += 1;
                }
                let child = internal.children[child_idx];
                if let Some(split) = self.insert_recursive(child, key, value, lsn, depth + 1)? {
                    internal.keys.insert(child_idx, split.separator);
                    internal.children.insert(child_idx + 1, split.right);
                } else {
                    return Ok(None);
                }

                if internal.keys.len() <= MAX_INTERNAL_KEYS {
                    self.pool.write(page_id, |p| {
                        p.set_page_lsn(lsn);
                        encode_node(p, &Node::Internal(internal))
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
                self.pool.write(page_id, |p| {
                    p.set_page_lsn(lsn);
                    encode_node(p, &Node::Internal(internal))
                })??;
                self.pool.write(right_page, |p| {
                    p.set_page_lsn(lsn);
                    encode_node(p, &Node::Internal(right))
                })??;
                Ok(Some(Split {
                    separator: promoted,
                    right: right_page,
                }))
            }
        }
    }
}
