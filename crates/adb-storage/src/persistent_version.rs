//! Version projection: immutable history indexed by `(RowId, begin_ts)`.
use std::{ops::Bound, path::Path};

use adb_btree::VersionBTree;
use adb_core::{CommitTs, KeyRange, Lsn, RowId, VersionKey};
use adb_journal::FileWrite;

use crate::{Checkpointable, HeapFile, HistoricalVersion, SpaceReuse, StorageError};

/// Temporal B+Tree over an append-only heap.
pub struct PersistentVersionStore {
    /// Serialized `HistoricalVersion`s.
    heap: HeapFile,
    /// `(RowId, begin_ts)` to heap location.
    index: VersionBTree,
}

impl PersistentVersionStore {
    /// Opens the store in `dir`.
    pub fn open(dir: impl AsRef<Path>, buffer_pages: usize) -> Result<Self, StorageError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            heap: HeapFile::open(
                dir.join("versions.heap"),
                buffer_pages,
                SpaceReuse::AppendOnly,
            )?,
            index: VersionBTree::open(
                dir.join("versions.idx"),
                dir.join("versions.idx.meta"),
                buffer_pages,
            )?,
        })
    }

    /// Records a historical version. Idempotent: a version that already exists is kept.
    pub fn put_at_lsn(
        &self,
        row_id: RowId,
        version: &HistoricalVersion,
        lsn: Lsn,
    ) -> Result<(), StorageError> {
        let key = VersionKey::new(row_id, version.begin_ts);
        if self.index.get(key)?.is_some() {
            return Ok(());
        }
        let location = self.heap.insert(&bincode::serialize(version)?, lsn)?;
        self.index.insert_at_lsn(key, location, lsn)?;
        Ok(())
    }

    /// Version of `row_id` visible at `ts`, if any.
    pub fn get_at(
        &self,
        row_id: RowId,
        ts: CommitTs,
    ) -> Result<Option<HistoricalVersion>, StorageError> {
        let Some((key, location)) = self.index.get_floor(VersionKey::new(row_id, ts))? else {
            return Ok(None);
        };
        if key.row_id != row_id {
            return Ok(None);
        }
        let version: HistoricalVersion = bincode::deserialize(&self.heap.read(location)?)?;
        Ok(version.visible_at(ts).then_some(version))
    }

    /// All versions of `row_id`, oldest first.
    pub fn history(&self, row_id: RowId) -> Result<Vec<HistoricalVersion>, StorageError> {
        self.index
            .scan(
                Bound::Included(VersionKey::new(row_id, CommitTs(0))),
                Bound::Included(VersionKey::new(row_id, CommitTs(u64::MAX))),
                usize::MAX,
            )?
            .into_iter()
            .map(|(_, location)| Ok(bincode::deserialize(&self.heap.read(location)?)?))
            .collect()
    }

    /// Up to `limit` distinct rows of `range` that have history, strictly after `after`.
    ///
    /// Each step is one index seek, so rows with long histories cost no more than short ones.
    pub fn row_ids(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
    ) -> Result<Vec<RowId>, StorageError> {
        let end = range.end.map_or(Bound::Unbounded, |end| {
            Bound::Excluded(VersionKey::new(end, CommitTs(0)))
        });
        let mut start = match range.bounds_after(after).0 {
            Bound::Excluded(row) => Bound::Excluded(VersionKey::new(row, CommitTs(u64::MAX))),
            _ => Bound::Included(VersionKey::new(range.start, CommitTs(0))),
        };
        let mut rows = Vec::new();
        while rows.len() < limit {
            let Some((key, _)) = self.index.scan(start, end, 1)?.into_iter().next() else {
                break;
            };
            rows.push(key.row_id);
            start = Bound::Excluded(VersionKey::new(key.row_id, CommitTs(u64::MAX)));
        }
        Ok(rows)
    }

    /// Every version (diagnostics and integrity checks only).
    pub fn scan_all(&self) -> Result<Vec<(VersionKey, HistoricalVersion)>, StorageError> {
        self.index
            .scan_all()?
            .into_iter()
            .map(|(key, location)| Ok((key, bincode::deserialize(&self.heap.read(location)?)?)))
            .collect()
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

impl Checkpointable for PersistentVersionStore {
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
