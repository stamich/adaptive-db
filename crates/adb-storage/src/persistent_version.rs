//! Module `persistent_version` for crate `adb-storage`.
use std::path::Path;

use adb_btree::VersionBTree;
use adb_core::{CommitTs, Lsn, RowId, RowLocation, VersionKey};

use crate::{HeapFile, HistoricalVersion, StorageError};

/// Represents `PersistentVersionStore` state used by this subsystem.
pub struct PersistentVersionStore {
    heap: HeapFile,
    index: VersionBTree,
}

/// Implements behavior for `PersistentVersionStore`.
impl PersistentVersionStore {
    /// Implements the `open` operation used by this subsystem.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;

        Ok(Self {
            heap: HeapFile::open(dir.join("versions.heap"), 128)?,
            index: VersionBTree::open(
                dir.join("versions.idx"),
                dir.join("versions.idx.meta"),
                128,
            )?,
        })
    }

    /// Implements the `put_at_lsn` operation used by this subsystem.
    pub fn put_at_lsn(
        &self,
        row_id: RowId,
        version: &HistoricalVersion,
        lsn: Lsn,
    ) -> Result<RowLocation, StorageError> {
        let key = VersionKey::new(row_id, version.begin_ts);

        // Logical idempotence: when the version key already exists, the
        // history fact has already been materialized.
        if let Some(location) = self.index.get(key)? {
            return Ok(location);
        }

        let bytes = bincode::serialize(version)?;
        let location = self.heap.insert(&bytes, lsn)?;
        self.index.insert_at_lsn(key, location, lsn)?;

        Ok(location)
    }

    /// Implements the `get_at` operation used by this subsystem.
    pub fn get_at(
        &self,
        row_id: RowId,
        ts: CommitTs,
    ) -> Result<Option<HistoricalVersion>, StorageError> {
        let search_key = VersionKey::new(row_id, ts);

        let Some((key, location)) = self.index.get_floor(search_key)? else {
            return Ok(None);
        };

        if key.row_id != row_id {
            return Ok(None);
        }

        let bytes = self.heap.read(location)?;
        let version: HistoricalVersion = bincode::deserialize(&bytes)?;

        Ok(version.visible_at(ts).then_some(version))
    }

    /// Implements the `history` operation used by this subsystem.
    pub fn history(&self, row_id: RowId) -> Result<Vec<HistoricalVersion>, StorageError> {
        self.index
            .range_for_row(row_id)?
            .into_iter()
            .map(|(_, location)| {
                let bytes = self.heap.read(location)?;
                Ok(bincode::deserialize(&bytes)?)
            })
            .collect()
    }

    /// Implements the `scan_all` operation used by this subsystem.
    pub fn scan_all(&self) -> Result<Vec<(VersionKey, HistoricalVersion)>, StorageError> {
        self.index
            .scan_all()?
            .into_iter()
            .map(|(key, location)| {
                let bytes = self.heap.read(location)?;
                let version = bincode::deserialize(&bytes)?;
                Ok((key, version))
            })
            .collect()
    }

    /// Implements the `flush` operation used by this subsystem.
    pub fn flush(&self) -> Result<(), StorageError> {
        self.heap.flush()?;
        self.index.flush()?;
        Ok(())
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
