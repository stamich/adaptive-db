//! Module `version_tree` for crate `adb-btree`.
use std::{path::Path, sync::Arc};

use adb_buffer::{BufferPool, FilePageStore};
use adb_core::{Lsn, PageId, RowId, RowLocation, VersionKey};
use adb_page::PageKind;
use parking_lot::{Mutex, RwLock};

use crate::{
    error::BTreeError,
    meta::MetaStore,
    version_codec::{decode_version_node, encode_version_node},
    version_node::{
        VersionInternalNode, VersionLeafNode, VersionNode, MAX_VERSION_INTERNAL_KEYS,
        MAX_VERSION_LEAF_ENTRIES,
    },
};

/// Represents `VersionSplit` state used by this subsystem.
struct VersionSplit {
    separator: VersionKey,
    right: PageId,
}

/// Represents `VersionBTree` state used by this subsystem.
pub struct VersionBTree {
    pool: Arc<BufferPool>,
    root: RwLock<PageId>,
    meta: MetaStore,
    tree_lock: Mutex<()>,
}

/// Implements behavior for `VersionBTree`.
impl VersionBTree {
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
                    encode_version_node(page, &VersionNode::Leaf(VersionLeafNode::empty()))
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
    pub fn get(&self, key: VersionKey) -> Result<Option<RowLocation>, BTreeError> {
        let _guard = self.tree_lock.lock();
        let leaf = self.find_leaf(key)?;

        Ok(leaf
            .keys
            .binary_search(&key)
            .ok()
            .map(|index| leaf.values[index]))
    }

    /// Returns the greatest key <= requested key.
    pub fn get_floor(
        &self,
        key: VersionKey,
    ) -> Result<Option<(VersionKey, RowLocation)>, BTreeError> {
        let _guard = self.tree_lock.lock();
        let leaf = self.find_leaf(key)?;

        match leaf.keys.binary_search(&key) {
            Ok(index) => Ok(Some((leaf.keys[index], leaf.values[index]))),
            Err(0) => self.find_floor_before_leaf(key),
            Err(index) => Ok(Some((leaf.keys[index - 1], leaf.values[index - 1]))),
        }
    }

    /// Implements the `range_for_row` operation used by this subsystem.
    pub fn range_for_row(
        &self,
        row_id: RowId,
    ) -> Result<Vec<(VersionKey, RowLocation)>, BTreeError> {
        let _guard = self.tree_lock.lock();
        let start = VersionKey::new(row_id, adb_core::CommitTs(0));
        let mut page_id = self.find_leaf_page(start)?;
        let mut out = Vec::new();
        let max_steps = self.pool.page_count()?.saturating_add(1);
        let mut steps = 0u64;

        loop {
            steps = steps.saturating_add(1);
            if steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected in version leaf chain".into(),
                ));
            }
            let node = self.pool.read(page_id, decode_version_node)??;
            let VersionNode::Leaf(leaf) = node else {
                return Err(BTreeError::Corrupt(
                    "leaf chain points to an internal node".to_string(),
                ));
            };

            for (key, value) in leaf.keys.iter().copied().zip(leaf.values.iter().copied()) {
                if key.row_id == row_id {
                    out.push((key, value));
                } else if key.row_id > row_id {
                    return Ok(out);
                }
            }

            match leaf.next {
                Some(next) => page_id = next,
                None => return Ok(out),
            }
        }
    }

    /// Implements the `scan_all` operation used by this subsystem.
    pub fn scan_all(&self) -> Result<Vec<(VersionKey, RowLocation)>, BTreeError> {
        let _guard = self.tree_lock.lock();
        let mut page_id = *self.root.read();
        let max_steps = self.pool.page_count()?.saturating_add(1);
        let mut descent_steps = 0u64;

        loop {
            descent_steps = descent_steps.saturating_add(1);
            if descent_steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected while locating first version leaf".into(),
                ));
            }
            match self.pool.read(page_id, decode_version_node)?? {
                VersionNode::Leaf(_) => break,
                VersionNode::Internal(internal) => {
                    page_id = internal.children[0];
                }
            }
        }

        let mut out = Vec::new();
        let mut leaf_steps = 0u64;

        loop {
            leaf_steps = leaf_steps.saturating_add(1);
            if leaf_steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected in version leaf chain".into(),
                ));
            }
            let node = self.pool.read(page_id, decode_version_node)??;
            let VersionNode::Leaf(leaf) = node else {
                return Err(BTreeError::Corrupt(
                    "leaf chain points to an internal node".to_string(),
                ));
            };

            out.extend(leaf.keys.iter().copied().zip(leaf.values.iter().copied()));

            match leaf.next {
                Some(next) => page_id = next,
                None => return Ok(out),
            }
        }
    }

    /// Implements the `insert_at_lsn` operation used by this subsystem.
    pub fn insert_at_lsn(
        &self,
        key: VersionKey,
        value: RowLocation,
        lsn: Lsn,
    ) -> Result<(), BTreeError> {
        let _guard = self.tree_lock.lock();
        let root = *self.root.read();

        if let Some(split) = self.insert_recursive(root, key, value, lsn, 0)? {
            let new_root = self.pool.allocate_page(PageKind::BTreeInternal)?;

            let node = VersionNode::Internal(VersionInternalNode {
                keys: vec![split.separator],
                children: vec![root, split.right],
            });

            self.pool.write(new_root, |page| {
                page.set_page_lsn(lsn);
                encode_version_node(page, &node)
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

    /// Implements the `find_leaf` operation used by this subsystem.
    fn find_leaf(&self, key: VersionKey) -> Result<VersionLeafNode, BTreeError> {
        let page_id = self.find_leaf_page(key)?;
        let node = self.pool.read(page_id, decode_version_node)??;

        match node {
            VersionNode::Leaf(leaf) => Ok(leaf),
            VersionNode::Internal(_) => {
                Err(BTreeError::Corrupt("expected a leaf page".to_string()))
            }
        }
    }

    /// Implements the `find_leaf_page` operation used by this subsystem.
    fn find_leaf_page(&self, key: VersionKey) -> Result<PageId, BTreeError> {
        let mut page_id = *self.root.read();
        let max_steps = self.pool.page_count()?.saturating_add(1);
        let mut steps = 0u64;

        loop {
            steps = steps.saturating_add(1);
            if steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected while descending version B+Tree".into(),
                ));
            }
            let node = self.pool.read(page_id, decode_version_node)??;

            match node {
                VersionNode::Leaf(_) => return Ok(page_id),

                VersionNode::Internal(internal) => {
                    let mut index = 0;

                    while index < internal.keys.len() && key >= internal.keys[index] {
                        index += 1;
                    }

                    page_id = internal.children[index];
                }
            }
        }
    }

    /// Slow predecessor fallback used only when the target leaf has no
    /// key <= requested key. Milestone 1.6 intentionally favors correctness
    /// over an additional previous-leaf pointer.
    fn find_floor_before_leaf(
        &self,
        key: VersionKey,
    ) -> Result<Option<(VersionKey, RowLocation)>, BTreeError> {
        let mut best = None;

        for (candidate, location) in self.scan_all_unlocked()? {
            if candidate <= key {
                best = Some((candidate, location));
            } else {
                break;
            }
        }

        Ok(best)
    }

    /// Implements the `scan_all_unlocked` operation used by this subsystem.
    fn scan_all_unlocked(&self) -> Result<Vec<(VersionKey, RowLocation)>, BTreeError> {
        let mut page_id = *self.root.read();
        let max_steps = self.pool.page_count()?.saturating_add(1);
        let mut descent_steps = 0u64;

        loop {
            descent_steps = descent_steps.saturating_add(1);
            if descent_steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected while locating first version leaf".into(),
                ));
            }
            match self.pool.read(page_id, decode_version_node)?? {
                VersionNode::Leaf(_) => break,
                VersionNode::Internal(internal) => {
                    page_id = internal.children[0];
                }
            }
        }

        let mut out = Vec::new();
        let mut leaf_steps = 0u64;

        loop {
            leaf_steps = leaf_steps.saturating_add(1);
            if leaf_steps > max_steps {
                return Err(BTreeError::Corrupt(
                    "cycle detected in version leaf chain".into(),
                ));
            }
            let node = self.pool.read(page_id, decode_version_node)??;
            let VersionNode::Leaf(leaf) = node else {
                return Err(BTreeError::Corrupt(
                    "leaf chain points to an internal node".to_string(),
                ));
            };

            out.extend(leaf.keys.iter().copied().zip(leaf.values.iter().copied()));

            match leaf.next {
                Some(next) => page_id = next,
                None => return Ok(out),
            }
        }
    }

    /// Implements the `insert_recursive` operation used by this subsystem.
    fn insert_recursive(
        &self,
        page_id: PageId,
        key: VersionKey,
        value: RowLocation,
        lsn: Lsn,
        depth: usize,
    ) -> Result<Option<VersionSplit>, BTreeError> {
        if depth > 1024 {
            return Err(BTreeError::Corrupt(
                "version B+Tree depth exceeds hardened limit".into(),
            ));
        }
        let node = self.pool.read(page_id, decode_version_node)??;

        match node {
            VersionNode::Leaf(mut leaf) => {
                match leaf.keys.binary_search(&key) {
                    Ok(index) => {
                        leaf.values[index] = value;

                        self.pool.write(page_id, |page| {
                            page.set_page_lsn(lsn);
                            encode_version_node(page, &VersionNode::Leaf(leaf))
                        })??;

                        return Ok(None);
                    }

                    Err(index) => {
                        leaf.keys.insert(index, key);
                        leaf.values.insert(index, value);
                    }
                }

                if leaf.keys.len() <= MAX_VERSION_LEAF_ENTRIES {
                    self.pool.write(page_id, |page| {
                        page.set_page_lsn(lsn);
                        encode_version_node(page, &VersionNode::Leaf(leaf))
                    })??;

                    return Ok(None);
                }

                let split_at = leaf.keys.len() / 2;
                let right_keys = leaf.keys.split_off(split_at);
                let right_values = leaf.values.split_off(split_at);

                let right_page = self.pool.allocate_page(PageKind::BTreeLeaf)?;

                let old_next = leaf.next;
                leaf.next = Some(right_page);

                let right = VersionLeafNode {
                    keys: right_keys,
                    values: right_values,
                    next: old_next,
                };

                let separator = right.keys[0];

                self.pool.write(page_id, |page| {
                    page.set_page_lsn(lsn);
                    encode_version_node(page, &VersionNode::Leaf(leaf))
                })??;

                self.pool.write(right_page, |page| {
                    page.set_page_lsn(lsn);
                    encode_version_node(page, &VersionNode::Leaf(right))
                })??;

                Ok(Some(VersionSplit {
                    separator,
                    right: right_page,
                }))
            }

            VersionNode::Internal(mut internal) => {
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

                if internal.keys.len() <= MAX_VERSION_INTERNAL_KEYS {
                    self.pool.write(page_id, |page| {
                        page.set_page_lsn(lsn);
                        encode_version_node(page, &VersionNode::Internal(internal))
                    })??;

                    return Ok(None);
                }

                let mid = internal.keys.len() / 2;
                let promoted = internal.keys[mid];

                let right_keys = internal.keys.split_off(mid + 1);
                internal.keys.truncate(mid);

                let right_children = internal.children.split_off(mid + 1);

                let right_page = self.pool.allocate_page(PageKind::BTreeInternal)?;

                let right = VersionInternalNode {
                    keys: right_keys,
                    children: right_children,
                };

                self.pool.write(page_id, |page| {
                    page.set_page_lsn(lsn);
                    encode_version_node(page, &VersionNode::Internal(internal))
                })??;

                self.pool.write(right_page, |page| {
                    page.set_page_lsn(lsn);
                    encode_version_node(page, &VersionNode::Internal(right))
                })??;

                Ok(Some(VersionSplit {
                    separator: promoted,
                    right: right_page,
                }))
            }
        }
    }
}
