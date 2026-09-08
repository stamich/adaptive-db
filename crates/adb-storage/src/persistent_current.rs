//! Persistent Current module for the adb-storage crate.
//!
use std::path::Path;

use adb_btree::BTree;
use adb_core::{Lsn, RowId, RowLocation};

use crate::{CurrentRecord, HeapFile, StorageError};

/// Persists the latest committed row version using a heap file plus a RowId B+Tree.
pub struct PersistentCurrentStore {
    heap: HeapFile,
    index: BTree,
}

/// Implements behavior for `PersistentCurrentStore`.
impl PersistentCurrentStore {
    /// Opens or creates the underlying resource and reconstructs the runtime state required by this subsystem.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            heap: HeapFile::open(dir.join("current.heap"), 128)?,
            index: BTree::open(dir.join("current.idx"), dir.join("current.idx.meta"), 128)?,
        })
    }

    /// Returns the value visible for the requested key or row at the operation's default snapshot.
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

    /// Flushes dirty state to the backing store and performs the subsystem's durability synchronization.
    pub fn flush(&self) -> Result<(), StorageError> {
        self.heap.flush()?;
        self.index.flush()?;
        Ok(())
    }
}
