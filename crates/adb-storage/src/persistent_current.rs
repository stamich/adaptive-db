//! Current-state projection: `RowId -> latest CurrentRecord`.
use std::path::Path;

use adb_btree::BTree;
use adb_core::{KeyRange, Lsn, RowId};
use adb_journal::FileWrite;

use crate::{Checkpointable, CurrentRecord, HeapFile, SpaceReuse, StorageError};

/// Primary B+Tree over a space-reclaiming heap.
pub struct PersistentCurrentStore {
    /// Serialized `CurrentRecord`s.
    heap: HeapFile,
    /// `RowId` to heap location.
    index: BTree,
}

impl PersistentCurrentStore {
    /// Opens the store in `dir`, caching up to `buffer_pages` clean pages per file.
    pub fn open(dir: impl AsRef<Path>, buffer_pages: usize) -> Result<Self, StorageError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            heap: HeapFile::open(dir.join("current.heap"), buffer_pages, SpaceReuse::Reclaim)?,
            index: BTree::open(
                dir.join("current.idx"),
                dir.join("current.idx.meta"),
                buffer_pages,
            )?,
        })
    }

    /// Latest record of `row_id` (including tombstones).
    pub fn get(&self, row_id: RowId) -> Result<Option<CurrentRecord>, StorageError> {
        match self.index.get(row_id)? {
            Some(location) => Ok(Some(bincode::deserialize(&self.heap.read(location)?)?)),
            None => Ok(None),
        }
    }

    /// Replaces the record of `row_id`, freeing the previous tuple.
    pub fn put_at_lsn(
        &self,
        row_id: RowId,
        record: &CurrentRecord,
        lsn: Lsn,
    ) -> Result<(), StorageError> {
        let bytes = bincode::serialize(record)?;
        if let Some(old) = self.index.get(row_id)? {
            self.heap.delete(old, lsn)?;
        }
        let location = self.heap.insert(&bytes, lsn)?;
        self.index.insert_at_lsn(row_id, location, lsn)?;
        Ok(())
    }

    /// Removes `row_id` entirely (used by vacuum for expired tombstones).
    pub fn remove(&self, row_id: RowId, lsn: Lsn) -> Result<bool, StorageError> {
        match self.index.remove(row_id, lsn)? {
            Some(location) => {
                self.heap.delete(location, lsn)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Up to `limit` records of `range` with keys strictly after `after`, in key order.
    pub fn scan(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
    ) -> Result<Vec<(RowId, CurrentRecord)>, StorageError> {
        let (start, end) = range.bounds_after(after);
        self.index
            .scan(start, end, limit)?
            .into_iter()
            .map(|(row_id, location)| {
                Ok((row_id, bincode::deserialize(&self.heap.read(location)?)?))
            })
            .collect()
    }

    /// Every record (diagnostics and integrity checks only).
    pub fn entries(&self) -> Result<Vec<(RowId, CurrentRecord)>, StorageError> {
        self.scan(&KeyRange::all(), None, usize::MAX)
    }

    /// Index pages.
    pub fn index_page_count(&self) -> u64 {
        self.index.page_count()
    }

    /// Heap pages.
    pub fn heap_page_count(&self) -> u64 {
        self.heap.page_count()
    }

    /// Persists everything directly (standalone use; the engine checkpoints via the journal).
    pub fn flush(&self) -> Result<(), StorageError> {
        self.heap.flush()?;
        Ok(self.index.flush()?)
    }
}

impl Checkpointable for PersistentCurrentStore {
    /// Dirty heap and index pages plus the index root.
    fn journal_writes(&self) -> Result<Vec<FileWrite>, StorageError> {
        let mut writes = self.heap.journal_writes()?;
        writes.extend(self.index.journal_writes()?);
        Ok(writes)
    }

    /// Marks heap and index persisted.
    fn mark_clean(&self) {
        self.heap.mark_clean();
        self.index.mark_clean();
    }

    /// Dirty heap and index pages.
    fn dirty_pages(&self) -> usize {
        self.heap.dirty_pages() + self.index.dirty_page_count()
    }
}
