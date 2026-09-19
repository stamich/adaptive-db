//! Module `persistent_current` for crate `adb-storage`.
use std::path::Path;

use adb_btree::BTree;
use adb_core::{Lsn, RowId, RowLocation};

use crate::{CurrentRecord, HeapFile, StorageError};

/// Represents `PersistentCurrentStore` state used by this subsystem.
pub struct PersistentCurrentStore {
    heap: HeapFile,
    index: BTree,
}

/// Implements behavior for `PersistentCurrentStore`.
impl PersistentCurrentStore {
    /// Implements the `open` operation used by this subsystem.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            heap: HeapFile::open(dir.join("current.heap"), 128)?,
            index: BTree::open(dir.join("current.idx"), dir.join("current.idx.meta"), 128)?,
        })
    }

    /// Implements the `get` operation used by this subsystem.
    pub fn get(&self, row_id: RowId) -> Result<Option<CurrentRecord>, StorageError> {
        let Some(location) = self.index.get(row_id)? else {
            return Ok(None);
        };
        let bytes = self.heap.read(location)?;
        Ok(Some(bincode::deserialize(&bytes)?))
    }

    /// Implements the `put` operation used by this subsystem.
    pub fn put(&self, row_id: RowId, record: &CurrentRecord) -> Result<RowLocation, StorageError> {
        self.put_at_lsn(row_id, record, Lsn(0))
    }

    /// Implements the `put_at_lsn` operation used by this subsystem.
    pub fn put_at_lsn(
        &self,
        row_id: RowId,
        record: &CurrentRecord,
        lsn: Lsn,
    ) -> Result<RowLocation, StorageError> {
        let bytes = bincode::serialize(record)?;
        let location = self.heap.insert(&bytes, lsn)?;
        self.index.insert_at_lsn(row_id, location, lsn)?;
        Ok(location)
    }

    /// Implements the `flush` operation used by this subsystem.
    pub fn flush(&self) -> Result<(), StorageError> {
        self.heap.flush()?;
        self.index.flush()?;
        Ok(())
    }

    /// Implements the `entries` operation used by this subsystem.
    pub fn entries(&self) -> Result<Vec<(RowId, CurrentRecord)>, StorageError> {
        self.index
            .scan_all()?
            .into_iter()
            .map(|(row_id, location)| {
                let bytes = self.heap.read(location)?;
                let record = bincode::deserialize(&bytes)?;
                Ok((row_id, record))
            })
            .collect()
    }

    /// Implements the `root_page_id` operation used by this subsystem.
    pub fn root_page_id(&self) -> adb_core::PageId {
        self.index.root_page_id()
    }

    /// Implements the `index_page_count` operation used by this subsystem.
    pub fn index_page_count(&self) -> Result<u64, StorageError> {
        Ok(self.index.page_count()?)
    }

    /// Implements the `heap_page_count` operation used by this subsystem.
    pub fn heap_page_count(&self) -> Result<u64, StorageError> {
        self.heap.page_count()
    }
}
