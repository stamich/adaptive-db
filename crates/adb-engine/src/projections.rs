//! The persistent projections of the log, treated as one unit.
//!
//! Applying a committed transaction, answering snapshot reads, paging scans, vacuuming
//! tombstones and producing checkpoint writes all go through [`Projections`], so the rest of
//! the engine never touches the individual stores.
//!
//! Since 2.3.0 the unit also holds the index catalog (see [`crate::index_catalog`]) and one
//! [`IndexHandle`] per active index. Every committed transaction is applied to the current
//! state, the version store and every index in the same step, under the same lock, and all of
//! them are published by the same checkpoint: an index is never behind or ahead of its table.

use std::path::{Path, PathBuf};

use adb_core::{CommitTs, KeyRange, Lsn, Row, RowId};
use adb_journal::FileWrite;
use adb_storage::{
    Checkpointable, CurrentRecord, HistoricalVersion, IntegrityChecker, IntegrityReport,
    PersistentCurrentStore, PersistentVersionStore, StorageError, StorageStats,
};
use adb_tx::Mutation;

use crate::{
    committed::CommittedTx,
    index_catalog::{index_dir, remove_orphans, IndexCatalog, INDEXES_DIR},
    projection::{IndexHandle, Projection},
    DbError,
};
use adb_index::{IndexError, IndexProjection, IndexState};

/// Current state plus history (and, since 2.3.0, the index catalog).
pub struct Projections {
    /// Database directory.
    dir: PathBuf,
    /// Latest state of every row.
    current: PersistentCurrentStore,
    /// Superseded row states.
    versions: PersistentVersionStore,
    /// Definitions of the secondary indexes.
    catalog: IndexCatalog,
    /// One projection per active index, in catalog order.
    indexes: Vec<IndexHandle>,
}

/// Directory of the current-state projection inside a database directory.
pub const CURRENT_DIR: &str = "current";
/// Directory of the version projection inside a database directory.
pub const VERSIONS_DIR: &str = "versions";

impl Projections {
    /// Opens the projections and the index catalog of the database in `dir`, removing index
    /// directories the catalog does not name.
    pub fn open(dir: &Path, buffer_pages: usize) -> Result<Self, DbError> {
        std::fs::create_dir_all(dir.join(INDEXES_DIR))?;
        let catalog = IndexCatalog::load(dir)?;
        remove_orphans(dir, &catalog)?;
        let mut indexes = Vec::new();
        for definition in catalog.definitions() {
            if definition.state == IndexState::Active {
                let projection = IndexProjection::open(
                    index_dir(dir, definition.id),
                    definition.clone(),
                    buffer_pages,
                    CommitTs(0),
                )?;
                indexes.push(IndexHandle::new(projection, CommitTs(0)));
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            current: PersistentCurrentStore::open(dir.join(CURRENT_DIR), buffer_pages)?,
            versions: PersistentVersionStore::open(dir.join(VERSIONS_DIR), buffer_pages)?,
            catalog,
            indexes,
        })
    }

    /// Restores the watermark and vacuum horizon of every index after opening: the persisted
    /// state reflects every commit up to `checkpointed_ts` and was vacuumed through
    /// `vacuumed_through` (replayed commits then move the watermarks forward).
    pub fn restore_horizons(&self, checkpointed_ts: CommitTs, vacuumed_through: CommitTs) {
        for index in &self.indexes {
            index.projection.set_watermark(checkpointed_ts);
            index.restore_vacuum_horizon(vacuumed_through);
        }
    }

    /// The active index projections, in catalog order.
    pub fn indexes(&self) -> &[IndexHandle] {
        &self.indexes
    }

    /// Checks, before a commit is logged, that every row it writes fits every index of its
    /// entity (a row that could not be indexed would otherwise fail after the commit point).
    pub fn check<'a>(
        &self,
        mutations: impl IntoIterator<Item = (&'a RowId, &'a Mutation)> + Clone,
    ) -> Result<(), DbError> {
        for index in &self.indexes {
            index.check(mutations.clone())?;
        }
        Ok(())
    }

    /// The index catalog.
    pub fn catalog(&self) -> &IndexCatalog {
        &self.catalog
    }

    /// The journal write that persists the index catalog (part of every checkpoint).
    pub fn catalog_journal_write(&self) -> Result<FileWrite, DbError> {
        self.catalog.journal_write(&self.dir)
    }

    /// Latest record of a row, including tombstones.
    pub fn current(&self, row_id: RowId) -> Result<Option<CurrentRecord>, StorageError> {
        self.current.get(row_id)
    }

    /// Applies a committed transaction: history first, then the new current state, then every
    /// index.
    pub fn apply(&self, tx: &CommittedTx, lsn: Lsn) -> Result<(), DbError> {
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
        for index in &self.indexes {
            index.apply(tx, lsn)?;
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

    /// Removes delete tombstones committed at or before `horizon` and index entries that ended
    /// at or before it; returns how many of each.
    ///
    /// Safe because (a) no live transaction has a snapshot older than `horizon`, so none can
    /// need the tombstone for write-conflict detection, and (b) the version index still holds
    /// the deleted row's history for point and historical reads.
    pub fn vacuum(&self, horizon: CommitTs, lsn: Lsn) -> Result<(u64, u64), DbError> {
        const PAGE: usize = 1024;
        let mut removed = 0;
        let mut after = None;
        loop {
            let page = self.current.scan(&KeyRange::all(), after, PAGE)?;
            let Some((last, _)) = page.last() else {
                let mut entries = 0;
                for index in &self.indexes {
                    entries += index.vacuum(horizon, lsn)?;
                }
                return Ok((removed, entries));
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

    /// Temporal integrity check of the stores, plus, for every index, its entry invariants and
    /// the equality of its current entries with the live rows of its entity.
    pub fn verify(&self) -> Result<IntegrityReport, DbError> {
        let mut report = IntegrityChecker::verify(&self.current, &self.versions)?;
        for index in &self.indexes {
            if let Err(error) = index.verify() {
                report.errors.push(format!("{}: {error}", index.name()));
                continue;
            }
            let definition = index.definition();
            let mut expected = std::collections::BTreeMap::new();
            for (row_id, record) in
                self.current
                    .scan(&KeyRange::entity(definition.entity_id), None, usize::MAX)?
            {
                if let Some(row) = record.value {
                    expected.insert(
                        row_id,
                        (
                            definition.key_values(&row),
                            definition.included_values(&row),
                        ),
                    );
                }
            }
            let actual: std::collections::BTreeMap<_, _> = index
                .projection
                .open_entries()?
                .into_iter()
                .map(|entry| (entry.row_id, (entry.key, entry.included)))
                .collect();
            let differing = expected
                .keys()
                .chain(actual.keys())
                .filter(|row_id| !same_values(expected.get(*row_id), actual.get(*row_id)))
                .count();
            if differing > 0 {
                report.errors.push(format!(
                    "{}: {differing} rows differ between the index and the table; rebuild the projections",
                    index.name()
                ));
            }
        }
        Ok(report)
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
    /// Dirty pages and roots of both stores and of every index.
    fn journal_writes(&self) -> Result<Vec<FileWrite>, StorageError> {
        let mut writes = self.current.journal_writes()?;
        writes.extend(self.versions.journal_writes()?);
        for index in &self.indexes {
            writes.extend(
                index
                    .projection
                    .journal_writes()
                    .map_err(index_storage_error)?,
            );
        }
        Ok(writes)
    }

    /// Marks every store persisted.
    fn mark_clean(&self) {
        self.current.mark_clean();
        self.versions.mark_clean();
        for index in &self.indexes {
            index.projection.mark_clean();
        }
    }

    /// Dirty pages of every store.
    fn dirty_pages(&self) -> usize {
        self.current.dirty_pages()
            + self.versions.dirty_pages()
            + self
                .indexes
                .iter()
                .map(|index| index.projection.dirty_pages())
                .sum::<usize>()
    }
}

/// An index error raised while collecting checkpoint writes, as a storage error.
fn index_storage_error(error: IndexError) -> StorageError {
    match error {
        IndexError::Tree(error) => StorageError::BTree(error),
        IndexError::Io(error) => StorageError::Io(error),
        other => StorageError::Invalid(other.to_string()),
    }
}

/// Whether an expected and an indexed `(key, covered)` pair hold the same values; NaN equals
/// NaN and `-0.0` equals `0.0`, as the key encoding stores them.
fn same_values(
    expected: Option<&(Vec<adb_core::Value>, Vec<adb_core::Value>)>,
    actual: Option<&(Vec<adb_core::Value>, Vec<adb_core::Value>)>,
) -> bool {
    use adb_core::{encode_key, Direction, Value};
    let encode = |values: &[Value]| encode_key(values.iter().map(|value| (value, Direction::Asc)));
    match (expected, actual) {
        (Some((ek, ei)), Some((ak, ai))) => encode(ek) == encode(ak) && encode(ei) == encode(ai),
        (None, None) => true,
        _ => false,
    }
}
