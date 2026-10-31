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
    schema_registry, DbError,
};
use adb_core::IndexId;
use adb_index::{IndexDefinition, IndexError, IndexProjection, IndexSpec, IndexState};
use adb_schema::SchemaRegistry;

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
    /// Entity schemas and the catalog epoch (2.4.0): the projection of the log's catalog records.
    registry: SchemaRegistry,
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
            registry: schema_registry::load(dir)?,
        })
    }

    /// The schema registry.
    pub fn registry(&self) -> &SchemaRegistry {
        &self.registry
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

    /// Builds a new index from the current state (the caller holds the commit lock, so the
    /// current state is exactly the effect of every commit up to `created_at_ts`).
    ///
    /// The definition enters the catalog as `Building`, the entries of every live row of its
    /// entity are inserted (each beginning at the commit that wrote the row), and the index
    /// becomes `Active` and is maintained from the next commit on. Nothing is persisted here:
    /// the caller's checkpoint publishes catalog and index together. On failure (a row that
    /// cannot be indexed) the catalog and the files are left as before.
    pub fn build_index(
        &mut self,
        spec: IndexSpec,
        created_at_ts: CommitTs,
        lsn: Lsn,
        buffer_pages: usize,
    ) -> Result<IndexDefinition, DbError> {
        let definition = self
            .catalog
            .add(spec, IndexState::Building, created_at_ts)?;
        let path = index_dir(&self.dir, definition.id);
        let built = self.build_entries(&definition, &path, created_at_ts, lsn, buffer_pages);
        match built {
            Ok(handle) => {
                self.catalog.set_state(definition.id, IndexState::Active);
                self.indexes.push(handle);
                Ok(self
                    .catalog
                    .by_id(definition.id)
                    .expect("just added")
                    .clone())
            }
            Err(error) => {
                self.catalog.remove(definition.id);
                let _ = std::fs::remove_dir_all(&path);
                Err(error)
            }
        }
    }

    /// Opens an empty projection for `definition` in `path` and fills it from the live rows.
    fn build_entries(
        &self,
        definition: &IndexDefinition,
        path: &Path,
        created_at_ts: CommitTs,
        lsn: Lsn,
        buffer_pages: usize,
    ) -> Result<IndexHandle, DbError> {
        const PAGE: usize = 1024;
        let mut active = definition.clone();
        active.state = IndexState::Active;
        let projection = IndexProjection::open(path, active, buffer_pages, created_at_ts)?;
        let range = KeyRange::entity(definition.entity_id);
        let mut after = None;
        loop {
            let page = self.current.scan(&range, after, PAGE)?;
            let Some((last, _)) = page.last() else {
                break;
            };
            after = Some(*last);
            for (row_id, record) in page {
                if let Some(row) = record.value {
                    let row = self.adapt_latest(row_id, row);
                    projection.insert_current(row_id, &row, record.commit_ts, lsn)?;
                }
            }
        }
        projection.set_watermark(created_at_ts);
        Ok(IndexHandle::new(projection, CommitTs(0)))
    }

    /// Removes index `name` from the catalog and the maintained set; returns its definition and
    /// the directory of its files (removed by the caller after the checkpoint that publishes
    /// the catalog without it).
    pub fn drop_index(&mut self, name: &str) -> Result<(IndexDefinition, PathBuf), DbError> {
        let id = self
            .catalog
            .by_name(name)
            .map(|definition| definition.id)
            .ok_or_else(|| DbError::IndexNotFound(name.to_string()))?;
        let definition = self.catalog.remove(id).expect("found above");
        self.indexes.retain(|handle| handle.definition().id != id);
        Ok((definition, index_dir(&self.dir, id)))
    }

    /// The active index with id `id`.
    pub fn index(&self, id: IndexId) -> Option<&IndexHandle> {
        self.indexes
            .iter()
            .find(|handle| handle.definition().id == id)
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

    /// The journal writes that persist the index catalog and the schema registry (part of every
    /// checkpoint).
    pub fn catalog_journal_writes(&self) -> Result<Vec<FileWrite>, DbError> {
        Ok(vec![
            self.catalog.journal_write(&self.dir)?,
            schema_registry::journal_write(&self.dir, &self.registry)?,
        ])
    }

    /// Latest record of a row, including tombstones.
    pub fn current(&self, row_id: RowId) -> Result<Option<CurrentRecord>, StorageError> {
        self.current.get(row_id)
    }

    /// Applies a committed transaction: its catalog changes first, then history, the new current
    /// state and every index.
    pub fn apply(&mut self, tx: &CommittedTx, lsn: Lsn) -> Result<(), DbError> {
        for (epoch, event) in &tx.catalog {
            self.registry.apply(*epoch, event)?;
        }
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
        if !self.indexes.is_empty() {
            // Index keys are computed from rows at the latest schema version: written rows
            // already are; before-images written under an older version are adapted, exactly
            // as the build and every earlier maintenance step saw them.
            let adapted = self.adapt_before_images(tx);
            let index_tx = adapted.as_ref().unwrap_or(tx);
            for index in &self.indexes {
                index.apply(index_tx, lsn)?;
            }
        }
        Ok(())
    }

    /// A copy of `tx` whose before-images are adapted to the latest schema of their entities,
    /// or `None` when every before-image already is (the common case).
    fn adapt_before_images(&self, tx: &CommittedTx) -> Option<CommittedTx> {
        let stale = |row_id: &RowId, row: &Row| {
            self.registry
                .latest(row_id.entity_id())
                .is_some_and(|schema| row.schema_version.effective() != schema.version)
        };
        let any_stale = tx
            .before_images
            .iter()
            .any(|(row_id, version)| version.value.as_ref().is_some_and(|row| stale(row_id, row)));
        if !any_stale {
            return None;
        }
        let mut adapted = tx.clone();
        for (row_id, version) in &mut adapted.before_images {
            if let Some(row) = version.value.take() {
                version.value = Some(self.adapt_latest(*row_id, row));
            }
        }
        Some(adapted)
    }

    /// `row` of `row_id` at the latest schema of its entity (unchanged without a schema).
    fn adapt_latest(&self, row_id: RowId, row: Row) -> Row {
        self.registry.adapt(row_id.entity_id(), row, None)
    }

    /// The row as seen by a snapshot at `ts`.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Result<Option<Row>, StorageError> {
        let record = self.current.get(row_id)?;
        self.visible(row_id, record, ts)
    }

    /// Many rows as seen by a snapshot at `ts`; `row_ids` must ascend (one descent per leaf of
    /// the primary index).
    pub fn get_many_at(
        &self,
        row_ids: &[RowId],
        ts: CommitTs,
    ) -> Result<Vec<Option<Row>>, StorageError> {
        let records = self.current.get_many(row_ids)?;
        row_ids
            .iter()
            .zip(records)
            .map(|(row_id, record)| self.visible(*row_id, record, ts))
            .collect()
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
                    let row = self.adapt_latest(row_id, row);
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
