//! The two persistent projections of the log, treated as one unit.
//!
//! Applying a committed transaction, answering snapshot reads, paging scans, vacuuming
//! tombstones and producing checkpoint writes all go through [`Projections`], so the rest of
//! the engine never touches the individual stores.

use std::path::Path;

use adb_core::{CommitTs, KeyRange, Lsn, Row, RowId};
use adb_journal::FileWrite;
use adb_storage::{
    Checkpointable, CurrentRecord, HistoricalVersion, IntegrityChecker, IntegrityReport,
    PersistentCurrentStore, PersistentVersionStore, StorageError, StorageStats,
};
use adb_tx::Mutation;

use crate::committed::CommittedTx;

/// Current state plus history.
pub struct Projections {
    /// Latest state of every row.
    current: PersistentCurrentStore,
    /// Superseded row states.
    versions: PersistentVersionStore,
}

/// Directory of the current-state projection inside a database directory.
pub const CURRENT_DIR: &str = "current";
/// Directory of the version projection inside a database directory.
pub const VERSIONS_DIR: &str = "versions";

impl Projections {
    /// Opens both projections of the database in `dir`.
    pub fn open(dir: &Path, buffer_pages: usize) -> Result<Self, StorageError> {
        Ok(Self {
            current: PersistentCurrentStore::open(dir.join(CURRENT_DIR), buffer_pages)?,
            versions: PersistentVersionStore::open(dir.join(VERSIONS_DIR), buffer_pages)?,
        })
    }

    /// Latest record of a row, including tombstones.
    pub fn current(&self, row_id: RowId) -> Result<Option<CurrentRecord>, StorageError> {
        self.current.get(row_id)
    }

    /// Applies a committed transaction: history first, then the new current state.
    pub fn apply(&self, tx: &CommittedTx, lsn: Lsn) -> Result<(), StorageError> {
        for (row_id, version) in &tx.before_images {
            self.versions.put_at_lsn(*row_id, version, lsn)?;
        }
        for (row_id, mutation) in &tx.mutations {
            let value = match mutation {
                Mutation::Put(row) => Some(row.clone()),
                Mutation::Delete => None,
            };
            self.current.put_at_lsn(
                *row_id,
                &CurrentRecord {
                    commit_ts: tx.commit_ts,
                    value,
                },
                lsn,
            )?;
        }
        Ok(())
    }

    /// The row as seen by a snapshot at `ts`.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Result<Option<Row>, StorageError> {
        let record = self.current.get(row_id)?;
        self.visible(row_id, record, ts)
    }

    /// Full history of a row, oldest first; the current state appears as an open interval.
    pub fn history(&self, row_id: RowId) -> Result<Vec<HistoricalVersion>, StorageError> {
        let mut history = self.versions.history(row_id)?;
        if let Some(current) = self.current.get(row_id)? {
            history.push(HistoricalVersion {
                begin_ts: current.commit_ts,
                end_ts: CommitTs(u64::MAX),
                value: current.value,
            });
        }
        Ok(history)
    }

    /// Up to `limit` rows of `range` visible at `ts`, strictly after `after`, in key order.
    ///
    /// Candidates come from the current index. When `include_history` is set (the snapshot is
    /// older than the vacuum horizon) rows that only survive in the version index are merged
    /// in, so vacuumed tombstones never hide rows from historical scans.
    pub fn scan_page(
        &self,
        range: &KeyRange,
        mut after: Option<RowId>,
        limit: usize,
        ts: CommitTs,
        include_history: bool,
    ) -> Result<Vec<(RowId, Row)>, StorageError> {
        let mut out = Vec::new();
        while out.len() < limit {
            let wanted = limit - out.len();
            let mut candidates: Vec<(RowId, Option<CurrentRecord>)> = self
                .current
                .scan(range, after, wanted)?
                .into_iter()
                .map(|(row_id, record)| (row_id, Some(record)))
                .collect();
            if include_history {
                for row_id in self.versions.row_ids(range, after, wanted)? {
                    if let Err(at) = candidates.binary_search_by_key(&row_id, |(id, _)| *id) {
                        candidates.insert(at, (row_id, None));
                    }
                }
                candidates.truncate(wanted);
            }
            if candidates.is_empty() {
                break;
            }
            for (row_id, record) in candidates {
                after = Some(row_id);
                let record = match record {
                    Some(record) => Some(record),
                    None => self.current.get(row_id)?,
                };
                if let Some(row) = self.visible(row_id, record, ts)? {
                    out.push((row_id, row));
                }
            }
        }
        Ok(out)
    }

    /// Removes delete tombstones committed at or before `horizon`; returns how many.
    ///
    /// Safe because (a) no live transaction has a snapshot older than `horizon`, so none can
    /// need the tombstone for write-conflict detection, and (b) the version index still holds
    /// the deleted row's history for point and historical reads.
    pub fn vacuum(&self, horizon: CommitTs, lsn: Lsn) -> Result<u64, StorageError> {
        const PAGE: usize = 1024;
        let mut removed = 0;
        let mut after = None;
        loop {
            let page = self.current.scan(&KeyRange::all(), after, PAGE)?;
            let Some((last, _)) = page.last() else {
                return Ok(removed);
            };
            after = Some(*last);
            for (row_id, record) in page {
                if record.value.is_none() && record.commit_ts <= horizon {
                    self.current.remove(row_id, lsn)?;
                    removed += 1;
                }
            }
        }
    }

    /// Counters of both projections.
    pub fn stats(&self) -> Result<StorageStats, StorageError> {
        let entries = self.current.entries()?;
        Ok(StorageStats {
            current_rows: entries.len() as u64,
            current_tombstones: entries
                .iter()
                .filter(|(_, record)| record.value.is_none())
                .count() as u64,
            historical_versions: self.versions.scan_all()?.len() as u64,
            current_heap_pages: self.current.heap_page_count(),
            current_index_pages: self.current.index_page_count(),
            version_heap_pages: self.versions.heap_page_count(),
            version_index_pages: self.versions.index_page_count(),
        })
    }

    /// Temporal integrity check.
    pub fn verify(&self) -> Result<IntegrityReport, StorageError> {
        IntegrityChecker::verify(&self.current, &self.versions)
    }

    /// Value of `row_id` at `ts`: the current record if committed by then, else the matching version.
    fn visible(
        &self,
        row_id: RowId,
        record: Option<CurrentRecord>,
        ts: CommitTs,
    ) -> Result<Option<Row>, StorageError> {
        if let Some(record) = record {
            if record.commit_ts <= ts {
                return Ok(record.value);
            }
        }
        Ok(self
            .versions
            .get_at(row_id, ts)?
            .and_then(|version| version.value))
    }
}

impl Checkpointable for Projections {
    /// Dirty pages and roots of both stores.
    fn journal_writes(&self) -> Result<Vec<FileWrite>, StorageError> {
        let mut writes = self.current.journal_writes()?;
        writes.extend(self.versions.journal_writes()?);
        Ok(writes)
    }

    /// Marks both stores persisted.
    fn mark_clean(&self) {
        self.current.mark_clean();
        self.versions.mark_clean();
    }

    /// Dirty pages of both stores.
    fn dirty_pages(&self) -> usize {
        self.current.dirty_pages() + self.versions.dirty_pages()
    }
}
