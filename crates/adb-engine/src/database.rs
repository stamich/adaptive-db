//! The database façade: transactions, queries, change feed and maintenance.
//!
//! # Durability model (Milestone 2.0.3)
//!
//! The canonical log is the source of truth. A commit is durable once its log records are
//! `fsync`ed; the projections (current and version stores) are updated in memory and reach
//! disk only through checkpoints, which publish all dirty pages and the checkpoint record
//! atomically via the journal. After a crash, recovery re-applies an interrupted checkpoint
//! and replays the log from the last checkpoint. Projections can always be rebuilt from the
//! log ([`Database::rebuild_projections`]).
//!
//! # Commit pipeline
//!
//! Under the commit lock: validate, allocate a commit timestamp, append the transaction's log
//! records, apply them to the projections. Outside the lock: wait for the log to be durable
//! (group commit), then publish the timestamp to new snapshots. Any failure after the first
//! log append poisons the instance and is reported as [`DbError::CommitOutcomeUnknown`].
//!
//! The same apply step adds the transaction's mutations to the per-entity modification
//! counters that measure how stale the optimizer statistics are (see [`crate::statistics`]).
//!
//! Lock order: commit lock → log → projections → modification counters → transaction manager.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use adb_core::encode_key;
use adb_core::{CommitTs, FieldId, IndexId, KeyRange, Row, RowId, SchemaEpoch};
use adb_execution::{
    DataSource, ExecutionContext, ExecutionError, Executor, IndexAccess, IndexPage, PhysicalPlan,
    QueryCursor, ReadView,
};
use adb_index::{IndexBounds, IndexDefinition, IndexSpec, ScanDirection};
use adb_journal::{Journal, JOURNAL_FILE};
use adb_schema::{CatalogEvent, EntitySchema, EntitySpec, SchemaChange, SchemaRegistry};
use adb_stats::{AnalyzeOptions, TableStatistics};
use adb_stats::{IndexCollector, IndexStatistics};
use adb_storage::{
    Checkpoint, CheckpointStore, Checkpointable, HistoricalVersion, IntegrityReport, StorageStats,
};
use adb_tx::{validate, IsolationLevel, Mutation, Transaction, TransactionManager};
use adb_wal::earliest_lsn;
use parking_lot::{Mutex, RwLock};

use crate::{
    cdc::{self, ChangeBatch, ChangeCursor, ChangeFilter},
    committed::CommittedTx,
    format,
    health::EngineHealth,
    index_catalog::INDEXES_DIR,
    log::LogWriter,
    offsets::ConsumerOffsets,
    projection::Projection,
    projections::{Projections, CURRENT_DIR, VERSIONS_DIR},
    recovery::{recover, write_checkpoint},
    schema_registry::registry_path,
    statistics::{modifications_path, ModificationCounters, StatisticsStore},
    DatabaseOptions, DbError,
};

/// Log directory inside the database directory.
const WAL_DIR: &str = "wal";
/// Checkpoint record file inside the database directory.
const CHECKPOINT_FILE: &str = "checkpoint.meta";

/// An index and the snapshots it can serve (see [`crate::projection::Projection::serves`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    /// The definition.
    pub definition: IndexDefinition,
    /// Oldest snapshot the index serves: `max(created_at_ts, vacuum horizon)`.
    pub usable_from: CommitTs,
    /// Latest commit the index reflects.
    pub watermark: CommitTs,
}

/// Result of [`Database::vacuum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VacuumReport {
    /// Tombstones removed from the current store.
    pub tombstones_removed: u64,
    /// Index entries removed (they ended at or before the horizon).
    pub index_entries_removed: u64,
    /// Tombstones committed and index entries ended at or before this timestamp were eligible.
    pub horizon: CommitTs,
}

/// A handle to one open database. Cheap to clone; all clones share the instance.
#[derive(Clone)]
pub struct Database {
    /// Shared state of the instance.
    inner: Arc<Inner>,
}

/// State shared by all clones of a [`Database`].
struct Inner {
    /// Database directory.
    dir: PathBuf,
    /// Configuration the instance was opened with.
    options: DatabaseOptions,
    /// Directory of the canonical log.
    wal_dir: PathBuf,
    /// Appender of the canonical log (group commit).
    log: LogWriter,
    /// Current and version stores; readers share, apply/vacuum take it exclusively.
    projections: RwLock<Projections>,
    /// Ids, timestamps and active snapshots.
    tx_manager: Mutex<TransactionManager>,
    /// Serializes validation, log append and apply of commits (and checkpoint/vacuum).
    commit_lock: Mutex<()>,
    /// Atomic publication of checkpoints.
    journal: Journal,
    /// Checkpoint record file.
    checkpoints: CheckpointStore,
    /// Durable change-feed cursors of named consumers.
    offsets: ConsumerOffsets,
    /// Poisoning state.
    health: EngineHealth,
    /// Snapshots older than this may need the version index to enumerate rows (see vacuum).
    vacuumed_through: AtomicU64,
    /// Row mutations committed per entity; updated by the commit apply step.
    modifications: Mutex<ModificationCounters>,
    /// Optimizer statistics written by `ANALYZE`.
    statistics: StatisticsStore,
}

impl Database {
    /// Opens (creating if needed) the database in `path` with default options.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        Self::open_with(path, DatabaseOptions::default())
    }

    /// Opens (creating if needed) the database in `path`.
    pub fn open_with(path: impl AsRef<Path>, options: DatabaseOptions) -> Result<Self, DbError> {
        let dir = path.as_ref();
        fs::create_dir_all(dir)?;
        let wal_dir = dir.join(WAL_DIR);
        // Refuse a directory written in another data format before touching anything in it.
        format::check_or_stamp(dir, &wal_dir)?;
        fs::create_dir_all(&wal_dir)?;
        let journal = Journal::new(dir);
        let checkpoints = CheckpointStore::new(dir.join(CHECKPOINT_FILE));

        let recovered = recover(dir, &wal_dir, &options, &journal, &checkpoints)?;
        let log = LogWriter::open(&wal_dir, options.log_segment_bytes)?;
        let mut tx_manager = TransactionManager::default();
        tx_manager.advance_after_recovery(recovered.last_tx_id, recovered.last_commit_ts.0);

        Ok(Self {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                options,
                wal_dir,
                log,
                projections: RwLock::new(recovered.projections),
                tx_manager: Mutex::new(tx_manager),
                commit_lock: Mutex::new(()),
                journal,
                checkpoints,
                offsets: ConsumerOffsets::open(dir)?,
                health: EngineHealth::default(),
                vacuumed_through: AtomicU64::new(recovered.vacuumed_through.0),
                modifications: Mutex::new(recovered.modifications),
                statistics: StatisticsStore::open(dir)?,
            }),
        })
    }

    /// Discards both projections and rebuilds them from the canonical log.
    ///
    /// Use after [`DbError::is_corruption`] reports a damaged store. Requires the complete log
    /// (it is never pruned by Milestone 2.0.3+). The database must not be open elsewhere.
    ///
    /// The modification counters are recounted from the log too; statistics documents are
    /// kept (they describe a snapshot, not the store). Index projections are derived data and
    /// are rebuilt as well; the index catalog (their definitions) is kept. The schema registry
    /// is rebuilt from the catalog records of the log.
    pub fn rebuild_projections(
        path: impl AsRef<Path>,
        options: DatabaseOptions,
    ) -> Result<Self, DbError> {
        let dir = path.as_ref();
        if let Some(earliest) = earliest_lsn(dir.join(WAL_DIR))? {
            if earliest.0 != 0 {
                return Err(DbError::ChangeLogTruncated {
                    requested: adb_core::Lsn(0),
                    earliest,
                });
            }
        }
        for name in [CURRENT_DIR, VERSIONS_DIR] {
            let path = dir.join(name);
            if path.exists() {
                fs::remove_dir_all(path)?;
            }
        }
        let indexes = dir.join(INDEXES_DIR);
        if indexes.exists() {
            for entry in fs::read_dir(&indexes)? {
                let path = entry?.path();
                if path.is_dir() {
                    fs::remove_dir_all(path)?;
                }
            }
        }
        for path in [
            dir.join(CHECKPOINT_FILE),
            dir.join(JOURNAL_FILE),
            dir.join("checkpoint.journal.tmp"),
            modifications_path(dir),
            registry_path(dir),
        ] {
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Self::open_with(dir, options)
    }

    // ----- transactions -------------------------------------------------------------------

    /// Starts a serializable transaction.
    pub fn begin(&self) -> Transaction {
        self.begin_with(IsolationLevel::Serializable)
    }

    /// Starts a transaction with the given isolation level.
    pub fn begin_with(&self, isolation: IsolationLevel) -> Transaction {
        self.inner.tx_manager.lock().begin(isolation)
    }

    /// Reads a row inside `tx`: its own writes first, then its snapshot (through the latest
    /// schema, like [`Database::get_at`]). The read is recorded for serializable validation.
    ///
    /// Because the row comes back at the latest version, writing it back after a change stores
    /// it at that version: this is how rows migrate lazily, one update at a time.
    pub fn get_in_tx(&self, tx: &mut Transaction, row_id: RowId) -> Result<Option<Row>, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }
        if let Some(local) = tx.local_read(row_id) {
            return Ok(local);
        }
        tx.record_read(row_id);
        self.get_at(row_id, tx.snapshot_ts())
    }

    /// Validates and commits `tx`; returns its commit timestamp once it is durable.
    pub fn commit(&self, mut tx: Transaction) -> Result<CommitTs, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }
        tx.mark_closed();
        self.inner.health.check()?;
        if tx.is_read_only() {
            // A read-only transaction observed one consistent snapshot: nothing to validate.
            return Ok(tx.snapshot_ts());
        }

        let (commit_ts, end) = {
            let _commit = self.inner.commit_lock.lock();
            self.inner.health.check()?;
            let committed = self.prepare(&tx)?;
            let commit_ts = committed.commit_ts;
            let end = self
                .inner
                .health
                .guard(|| {
                    let appended = self.inner.log.append(&committed.to_records())?;
                    self.inner
                        .projections
                        .write()
                        .apply(&committed, appended.commit_lsn)?;
                    self.inner.modifications.lock().record(&committed);
                    Ok(appended.end)
                })
                .map_err(DbError::CommitOutcomeUnknown)?;
            (commit_ts, end)
        };

        self.inner
            .health
            .guard(|| Ok(self.inner.log.sync_through(end)?))
            .map_err(DbError::CommitOutcomeUnknown)?;
        self.inner.tx_manager.lock().publish_commit(commit_ts);
        self.checkpoint_if_needed();
        Ok(commit_ts)
    }

    /// Abandons `tx`. (Dropping it has the same effect.)
    pub fn rollback(&self, mut tx: Transaction) -> Result<(), DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }
        tx.mark_closed();
        Ok(())
    }

    // ----- reads ----------------------------------------------------------------------------

    /// Latest committed state of a row.
    pub fn get(&self, row_id: RowId) -> Result<Option<Row>, DbError> {
        self.get_at(row_id, self.latest_committed_ts())
    }

    /// State of a row at snapshot `ts`, read through the latest schema of its entity (a row
    /// written under an older version is adapted: virtual defaults, dropped fields hidden).
    /// Rows of entities without a schema are returned as stored; [`Database::history`] returns
    /// stored rows.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Result<Option<Row>, DbError> {
        self.inner.health.check()?;
        let projections = self.inner.projections.read();
        Ok(projections
            .get_at(row_id, ts)?
            .map(|row| projections.registry().adapt(row_id.entity_id(), row, None)))
    }

    /// Every stored version of a row, oldest first, as written (not adapted); the current one
    /// has an open end.
    pub fn history(&self, row_id: RowId) -> Result<Vec<HistoricalVersion>, DbError> {
        self.inner.health.check()?;
        Ok(self.inner.projections.read().history(row_id)?)
    }

    /// Latest commit visible to new snapshots.
    pub fn latest_committed_ts(&self) -> CommitTs {
        self.inner.tx_manager.lock().latest_committed_ts()
    }

    /// Runs a physical plan at the latest snapshot.
    pub fn execute(&self, plan: PhysicalPlan) -> Result<QueryCursor, DbError> {
        self.execute_at(plan, self.latest_committed_ts())
    }

    /// Runs a physical plan at snapshot `snapshot_ts`.
    pub fn execute_at(
        &self,
        plan: PhysicalPlan,
        snapshot_ts: CommitTs,
    ) -> Result<QueryCursor, DbError> {
        self.inner.health.check()?;
        // The query reads through the schemas current when it starts, whatever DDL commits
        // while it runs.
        let mut context = ExecutionContext::new(snapshot_ts);
        context.schema_epoch = Some(self.schema_epoch());
        Ok(Executor::execute(Arc::new(self.clone()), plan, context)?)
    }

    // ----- change data capture --------------------------------------------------------------

    /// Up to `max_events` committed transactions after `from` whose changes match `filter`.
    pub fn read_changes(
        &self,
        from: ChangeCursor,
        max_events: usize,
        filter: &ChangeFilter,
    ) -> Result<ChangeBatch, DbError> {
        self.inner.health.check()?;
        cdc::read_changes(
            &self.inner.wal_dir,
            from,
            self.inner.log.durable_end(),
            max_events,
            filter,
        )
    }

    /// Cursor at the durable end of the log (a consumer that only wants new changes).
    pub fn change_feed_end(&self) -> ChangeCursor {
        ChangeCursor(self.inner.log.durable_end())
    }

    /// Durably stores the cursor of the named consumer.
    pub fn commit_consumer_offset(&self, name: &str, cursor: ChangeCursor) -> Result<(), DbError> {
        if cursor.0 > self.inner.log.durable_end() {
            return Err(DbError::InvalidArgument(
                "consumer offset is beyond the durable end of the log".into(),
            ));
        }
        self.inner.offsets.commit(name, cursor)
    }

    /// Last committed cursor of the named consumer.
    pub fn consumer_offset(&self, name: &str) -> Option<ChangeCursor> {
        self.inner.offsets.get(name)
    }

    // ----- statistics -----------------------------------------------------------------------

    /// Collects optimizer statistics for `entity_id` and durably replaces its document.
    ///
    /// Scans one snapshot (the latest published commit) without blocking commits. The
    /// document records the entity's modification counter at that moment, so
    /// [`Database::modifications_since_analyze`] measures change since this run. Commits
    /// applied but not yet published when the run starts are counted as already analyzed;
    /// the counter is a staleness signal, not an exact delta. If a concurrent `ANALYZE` of the
    /// same entity already published a newer snapshot, that document stays in force.
    ///
    /// The entity id is not checked against a catalog (the engine has none): analyzing an
    /// entity without rows publishes an empty document.
    pub fn analyze(
        &self,
        entity_id: u64,
        options: &AnalyzeOptions,
    ) -> Result<TableStatistics, DbError> {
        self.inner.health.check()?;
        let (snapshot, modifications) = {
            let _commit = self.inner.commit_lock.lock();
            (
                self.latest_committed_ts(),
                self.inner.modifications.lock().get(entity_id),
            )
        };
        // Like a query, the scan reads the version index once vacuum passes its snapshot.
        let mut statistics = adb_stats::analyze(self, entity_id, snapshot, options)?;
        statistics.modifications_at_analyze = modifications;
        statistics.indexes = self.index_statistics(entity_id, snapshot)?;
        // The scan read rows through the latest schema (see `DataSource::scan_page`).
        statistics.schema_version = self.schema(entity_id).map_or(0, |schema| schema.version.0);
        self.inner.statistics.save(statistics.clone())?;
        Ok(statistics)
    }

    /// Statistics of every index of `entity_id` at `snapshot`, in id order: the entries
    /// visible at the snapshot are read in index order, one page per acquisition of the
    /// projection lock (commits proceed between pages), and the tree shape is read. An index
    /// dropped meanwhile is skipped.
    fn index_statistics(
        &self,
        entity_id: u64,
        snapshot: CommitTs,
    ) -> Result<Vec<IndexStatistics>, DbError> {
        const PAGE: usize = 1024;
        let indexes: Vec<IndexDefinition> = self
            .indexes()
            .into_iter()
            .map(|status| status.definition)
            .filter(|definition| definition.entity_id == entity_id)
            .collect();
        let mut out = Vec::new();
        'indexes: for definition in indexes {
            let directions = definition.directions();
            let mut collector = IndexCollector::default();
            let mut resume: Option<Vec<u8>> = None;
            loop {
                let projections = self.inner.projections.read();
                let Some(handle) = projections.index(definition.id) else {
                    continue 'indexes;
                };
                let page = handle.projection.scan(
                    &IndexBounds::all(),
                    snapshot,
                    ScanDirection::Forward,
                    resume.as_deref(),
                    PAGE,
                    PAGE,
                )?;
                for entry in &page.entries {
                    let prefixes: Vec<Vec<u8>> = (1..=entry.key.len())
                        .map(|length| {
                            encode_key(entry.key[..length].iter().zip(directions.iter().copied()))
                        })
                        .collect();
                    let borrowed: Vec<&[u8]> = prefixes.iter().map(Vec::as_slice).collect();
                    collector.add(&borrowed, entry.row_id.0);
                }
                match page.resume {
                    Some(key) => resume = Some(key),
                    None => {
                        let shape = handle.projection.shape()?;
                        out.push(collector.finish(
                            definition.id.0,
                            shape.entries,
                            shape.height,
                            shape.leaf_pages,
                        ));
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    /// The statistics document of `entity_id`, if it was analyzed.
    pub fn statistics(&self, entity_id: u64) -> Option<TableStatistics> {
        self.inner.statistics.get(entity_id)
    }

    /// Generation of the statistics document of `entity_id` (0 if there is none). It changes
    /// whenever `ANALYZE` publishes a new document, so a client can cache decoded documents and
    /// re-read them only when it changes.
    pub fn statistics_generation(&self, entity_id: u64) -> u64 {
        self.inner.statistics.generation(entity_id)
    }

    /// Row mutations committed to `entity_id` since its last `ANALYZE` (since creation, or
    /// since the upgrade to 2.2.3 for older databases, if it was never analyzed).
    pub fn modifications_since_analyze(&self, entity_id: u64) -> u64 {
        let total = self.inner.modifications.lock().get(entity_id);
        let at_analyze = self
            .inner
            .statistics
            .get(entity_id)
            .map_or(0, |statistics| statistics.modifications_at_analyze);
        total.saturating_sub(at_analyze)
    }

    // ----- secondary indexes ------------------------------------------------------------------

    /// Creates and builds a secondary index (`CREATE INDEX`); returns its definition.
    ///
    /// The build holds the commit lock (commits wait; queries wait while the entries are
    /// inserted): the index is filled from the current state, which then reflects exactly the
    /// commits up to `created_at_ts`, and a forced checkpoint publishes the catalog entry and
    /// the index pages in one journal commit before the index becomes visible. A crash before
    /// that commit leaves no trace (the half-built files are removed on the next open); after
    /// it, the index is complete and recovery maintains it from the log. The index serves
    /// snapshots from `created_at_ts` on.
    pub fn create_index(&self, spec: IndexSpec) -> Result<IndexDefinition, DbError> {
        self.inner.health.check()?;
        let _commit = self.inner.commit_lock.lock();
        self.inner.health.check()?;
        let created_at = self.inner.tx_manager.lock().last_allocated_commit_ts();
        let lsn = self.inner.log.end()?;
        let definition = self.inner.projections.write().build_index(
            spec,
            created_at,
            lsn,
            self.inner.options.buffer_pages,
        )?;
        // Plans made before the index existed must be planned again (they may now use it).
        self.commit_catalog_locked(|_, _| Ok((CatalogEvent::Epoch, ())))?;
        // If the build cannot be published, the failed checkpoint poisons the instance and the
        // next open starts without the index.
        self.checkpoint_locked()?;
        // Every commit up to `created_at` is applied and, after the checkpoint's log sync,
        // durable: it may be published, so a query planned right now can use the index.
        self.inner.tx_manager.lock().publish_commit(created_at);
        Ok(definition)
    }

    /// Drops the index `name` (`DROP INDEX`); returns its definition.
    ///
    /// The definition leaves the catalog and the index stops being maintained; a checkpoint
    /// publishes the catalog without it, then its files are removed (a crash in between leaves
    /// files that the next open removes). Queries still scanning it fail on their next page.
    pub fn drop_index(&self, name: &str) -> Result<IndexDefinition, DbError> {
        self.inner.health.check()?;
        let _commit = self.inner.commit_lock.lock();
        self.inner.health.check()?;
        let (definition, files) = self.inner.projections.write().drop_index(name)?;
        // Plans that scan the index must be planned again.
        self.commit_catalog_locked(|_, _| Ok((CatalogEvent::Epoch, ())))?;
        self.checkpoint_locked()?;
        if files.exists() {
            fs::remove_dir_all(files)?;
        }
        Ok(definition)
    }

    /// Every index with the range of snapshots it serves, in id order.
    pub fn indexes(&self) -> Vec<IndexStatus> {
        let projections = self.inner.projections.read();
        projections
            .catalog()
            .definitions()
            .iter()
            .map(|definition| {
                let handle = projections
                    .indexes()
                    .iter()
                    .find(|handle| handle.definition().id == definition.id);
                IndexStatus {
                    definition: definition.clone(),
                    usable_from: handle.map_or(CommitTs(u64::MAX), |h| h.usable_from()),
                    watermark: handle.map_or(CommitTs(0), |h| h.watermark()),
                }
            })
            .collect()
    }

    // ----- schemas ----------------------------------------------------------------------------

    /// The current catalog epoch: it moves with every change that can invalidate a plan.
    pub fn schema_epoch(&self) -> SchemaEpoch {
        self.inner.projections.read().registry().epoch()
    }

    /// The latest schema of the live entity `entity_id` (`None` for an entity without a
    /// registered schema: its rows are stored as written, without validation).
    pub fn schema(&self, entity_id: u64) -> Option<Arc<EntitySchema>> {
        self.inner
            .projections
            .read()
            .registry()
            .latest(entity_id)
            .cloned()
    }

    /// The latest schema of the live entity named `name` (case-insensitive).
    pub fn schema_by_name(&self, name: &str) -> Option<Arc<EntitySchema>> {
        self.inner
            .projections
            .read()
            .registry()
            .by_name(name)
            .cloned()
    }

    /// The latest schema of every live entity, in id order.
    pub fn schemas(&self) -> Vec<Arc<EntitySchema>> {
        self.inner
            .projections
            .read()
            .registry()
            .entities()
            .cloned()
            .collect()
    }

    /// Creates an entity (`CREATE TABLE`) and returns its version 1.
    ///
    /// The engine assigns the entity id (unless an imported catalog names one) and the field
    /// ids, validates the definition and its foreign-key hints, and commits a catalog
    /// transaction: the schema is durable in the log before it is visible, and every data
    /// commit after it is validated against it.
    pub fn create_entity(&self, spec: EntitySpec) -> Result<Arc<EntitySchema>, DbError> {
        let entity_id = self.commit_catalog(|projections, epoch| {
            let registry = projections.registry();
            let entity_id = spec.entity_id.unwrap_or(registry.next_entity_id());
            if registry.history(entity_id).is_some() {
                return Err(DbError::InvalidArgument(format!(
                    "entity id {entity_id} is already used"
                )));
            }
            let schema = adb_schema::create(&spec, entity_id, epoch)?;
            check_references(registry, &schema)?;
            Ok((CatalogEvent::Schema(schema), entity_id))
        })?;
        Ok(self.schema(entity_id).expect("just created"))
    }

    /// Applies one `ALTER TABLE` change to the live entity `entity_id` and returns the new
    /// version. Nothing is rewritten: rows keep the version they were written under and are
    /// adapted when read.
    pub fn alter_entity(
        &self,
        entity_id: u64,
        change: &SchemaChange,
    ) -> Result<Arc<EntitySchema>, DbError> {
        self.commit_catalog(|projections, epoch| {
            let registry = projections.registry();
            let current = registry
                .latest(entity_id)
                .ok_or_else(|| DbError::EntityNotFound(entity_id.to_string()))?;
            let next = adb_schema::apply(current, change, epoch)?;
            check_references(registry, &next)?;
            Ok((CatalogEvent::Schema(next), ()))
        })?;
        Ok(self.schema(entity_id).expect("just altered"))
    }

    /// Drops the live entity `entity_id` (`DROP TABLE`); returns its last version. Refused while
    /// indexes exist on it. Its rows stay in storage but are no longer reachable through the
    /// catalog, and its id is never reused.
    pub fn drop_entity(&self, entity_id: u64) -> Result<Arc<EntitySchema>, DbError> {
        let dropped = self
            .schema(entity_id)
            .ok_or_else(|| DbError::EntityNotFound(entity_id.to_string()))?;
        self.commit_catalog(|projections, _| {
            if projections.registry().latest(entity_id).is_none() {
                return Err(DbError::EntityNotFound(entity_id.to_string()));
            }
            let indexes: Vec<String> = projections
                .catalog()
                .definitions()
                .iter()
                .filter(|definition| definition.entity_id == entity_id)
                .map(|definition| definition.name.clone())
                .collect();
            if !indexes.is_empty() {
                return Err(DbError::InvalidArgument(format!(
                    "cannot drop {}: indexes {} depend on it (drop them first)",
                    dropped.name,
                    indexes.join(", ")
                )));
            }
            Ok((CatalogEvent::DropEntity { entity_id }, ()))
        })?;
        Ok(dropped)
    }

    /// Commits one catalog transaction: `build` sees the projections (registry and index
    /// catalog) and the epoch the change will publish and returns the event (plus a value for the caller). The event is validated
    /// against a copy of the registry before anything is logged, so a refused change leaves no
    /// trace and never poisons the instance.
    fn commit_catalog<T>(
        &self,
        build: impl FnOnce(&Projections, SchemaEpoch) -> Result<(CatalogEvent, T), DbError>,
    ) -> Result<T, DbError> {
        self.inner.health.check()?;
        let _commit = self.inner.commit_lock.lock();
        self.commit_catalog_locked(build)
    }

    /// Body of [`Database::commit_catalog`]; the caller holds the commit lock.
    fn commit_catalog_locked<T>(
        &self,
        build: impl FnOnce(&Projections, SchemaEpoch) -> Result<(CatalogEvent, T), DbError>,
    ) -> Result<T, DbError> {
        self.inner.health.check()?;
        let (epoch, event, value) = {
            let projections = self.inner.projections.read();
            let registry = projections.registry();
            let epoch = SchemaEpoch(registry.epoch().0 + 1);
            let (event, value) = build(&projections, epoch)?;
            let mut probe = SchemaRegistry::from_state(registry.state().clone())?;
            probe.apply(epoch, &event)?;
            (epoch, event, value)
        };
        let (tx_id, snapshot_ts, commit_ts) = {
            let mut tx_manager = self.inner.tx_manager.lock();
            let tx = tx_manager.begin(IsolationLevel::Serializable);
            (tx.id(), tx.snapshot_ts(), tx_manager.allocate_commit_ts())
        };
        let committed = CommittedTx {
            tx_id,
            snapshot_ts,
            commit_ts,
            before_images: Vec::new(),
            mutations: Vec::new(),
            catalog: vec![(epoch, event)],
        };
        let end = self
            .inner
            .health
            .guard(|| {
                let appended = self.inner.log.append(&committed.to_records())?;
                self.inner
                    .projections
                    .write()
                    .apply(&committed, appended.commit_lsn)?;
                Ok(appended.end)
            })
            .map_err(DbError::CommitOutcomeUnknown)?;
        self.inner
            .health
            .guard(|| Ok(self.inner.log.sync_through(end)?))
            .map_err(DbError::CommitOutcomeUnknown)?;
        self.inner.tx_manager.lock().publish_commit(commit_ts);
        Ok(value)
    }

    /// Every version of entity `entity_id` (live or dropped), oldest first.
    pub fn schema_history(&self, entity_id: u64) -> Vec<Arc<EntitySchema>> {
        self.inner
            .projections
            .read()
            .registry()
            .history(entity_id)
            .map(|history| history.versions.clone())
            .unwrap_or_default()
    }

    // ----- maintenance ----------------------------------------------------------------------

    /// Persists all in-memory projection changes and moves the recovery start point forward.
    pub fn checkpoint(&self) -> Result<(), DbError> {
        self.inner.health.check()?;
        let _commit = self.inner.commit_lock.lock();
        self.checkpoint_locked()
    }

    /// Body of [`Database::checkpoint`]; the caller holds the commit lock.
    fn checkpoint_locked(&self) -> Result<(), DbError> {
        self.inner.health.check()?;
        self.inner
            .health
            .guard(|| {
                let end = self.inner.log.end()?;
                self.inner.log.sync_through(end)?;
                let checkpoint = {
                    let tx_manager = self.inner.tx_manager.lock();
                    Checkpoint {
                        replay_from: end,
                        last_commit_ts: tx_manager.last_allocated_commit_ts(),
                        last_tx_id: tx_manager.last_tx_id(),
                        vacuumed_through: CommitTs(
                            self.inner.vacuumed_through.load(Ordering::Acquire),
                        ),
                        ..Checkpoint::default()
                    }
                };
                write_checkpoint(
                    &self.inner.dir,
                    &self.inner.journal,
                    &self.inner.checkpoints,
                    &self.inner.projections.read(),
                    &self.inner.modifications.lock(),
                    &checkpoint,
                )
            })
            .map_err(DbError::Poisoned)
    }

    /// Removes delete tombstones that no live transaction can still conflict with.
    ///
    /// Deleted rows remain readable at older snapshots through the version store; the change
    /// becomes durable with the next checkpoint.
    pub fn vacuum(&self) -> Result<VacuumReport, DbError> {
        self.inner.health.check()?;
        let _commit = self.inner.commit_lock.lock();
        self.inner.health.check()?;
        let horizon = self.inner.tx_manager.lock().oldest_active_snapshot();
        // Publish the horizon first: scans older than it start consulting the version index.
        self.inner
            .vacuumed_through
            .fetch_max(horizon.0, Ordering::AcqRel);
        let (tombstones, entries) = self
            .inner
            .health
            .guard(|| {
                let lsn = self.inner.log.end()?;
                self.inner.projections.write().vacuum(horizon, lsn)
            })
            .map_err(DbError::Poisoned)?;
        Ok(VacuumReport {
            tombstones_removed: tombstones,
            index_entries_removed: entries,
            horizon,
        })
    }

    /// Checkpoints if the instance is healthy. Dropping the last handle without calling this
    /// is safe; the next open replays the log instead.
    pub fn close(&self) -> Result<(), DbError> {
        self.checkpoint()
    }

    /// Whether an earlier failure poisoned this instance.
    pub fn is_poisoned(&self) -> bool {
        self.inner.health.is_poisoned()
    }

    /// Size counters of both projections.
    pub fn storage_stats(&self) -> Result<StorageStats, DbError> {
        Ok(self.inner.projections.read().stats()?)
    }

    /// Integrity check of every projection: temporal invariants of the stores, entry invariants
    /// of every index and the equality of each index with its table.
    pub fn verify(&self) -> Result<IntegrityReport, DbError> {
        self.inner.projections.read().verify()
    }

    /// Directory of the canonical log.
    pub fn wal_dir(&self) -> &Path {
        &self.inner.wal_dir
    }

    // ----- internals ------------------------------------------------------------------------

    /// Validates `tx` and builds what its commit will write. Caller holds the commit lock.
    fn prepare(&self, tx: &Transaction) -> Result<CommittedTx, DbError> {
        let projections = self.inner.projections.read();
        // Rows of entities with a schema are brought to its latest version and validated: the
        // engine, not the caller, decides what a valid row is.
        let mut mutations = Vec::with_capacity(tx.writes().len());
        for (row_id, mutation) in tx.writes() {
            mutations.push((
                *row_id,
                match mutation {
                    Mutation::Put(row) => {
                        Mutation::Put(projections.registry().prepare_write(*row_id, row.clone())?)
                    }
                    Mutation::Delete => Mutation::Delete,
                },
            ));
        }
        // Rows that cannot be indexed are refused here, before anything is logged.
        projections.check(
            mutations
                .iter()
                .map(|(row_id, mutation)| (row_id, mutation)),
        )?;
        let mut before = Vec::with_capacity(tx.writes().len());
        for row_id in tx.writes().keys() {
            before.push((*row_id, projections.current(*row_id)?));
        }
        let outcome = validate(tx, |row_id| -> Result<_, DbError> {
            Ok(match before.iter().find(|(id, _)| *id == row_id) {
                Some((_, record)) => record.as_ref().map(|record| record.commit_ts),
                None => projections.current(row_id)?.map(|record| record.commit_ts),
            })
        })?;
        outcome.map_err(DbError::TransactionConflict)?;

        let commit_ts = self.inner.tx_manager.lock().allocate_commit_ts();
        Ok(CommittedTx {
            tx_id: tx.id(),
            snapshot_ts: tx.snapshot_ts(),
            commit_ts,
            before_images: before
                .into_iter()
                .filter_map(|(row_id, record)| {
                    record.map(|record| {
                        (
                            row_id,
                            HistoricalVersion {
                                begin_ts: record.commit_ts,
                                end_ts: commit_ts,
                                value: record.value,
                            },
                        )
                    })
                })
                .collect(),
            mutations,
            catalog: Vec::new(),
        })
    }

    /// Starts a checkpoint once the dirty-page budget is exceeded. A failure poisons the
    /// instance (reported by the next operation); the commit that triggered it is durable.
    fn checkpoint_if_needed(&self) {
        let dirty = self.inner.projections.read().dirty_pages();
        if dirty > self.inner.options.checkpoint_dirty_pages {
            let _ = self.checkpoint();
        }
    }
}

impl DataSource for Database {
    /// Latest published commit (default snapshot of queries).
    fn latest_committed_ts(&self) -> CommitTs {
        Database::latest_committed_ts(self)
    }

    /// Snapshot read of one row for `PointLookup`, through the schemas of the view.
    fn point_lookup(&self, row_id: RowId, view: ReadView) -> Result<Option<Row>, ExecutionError> {
        let to_execution = |error: DbError| ExecutionError::DataSource(error.to_string());
        self.inner.health.check().map_err(to_execution)?;
        let projections = self.inner.projections.read();
        let row = projections
            .get_at(row_id, view.snapshot_ts)
            .map_err(|error| to_execution(error.into()))?;
        Ok(row.map(|row| {
            projections
                .registry()
                .adapt(row_id.entity_id(), row, view.schema_epoch)
        }))
    }

    /// Page of a range scan; merges the version index when the snapshot predates the vacuum
    /// horizon; rows are adapted to the schemas of the view.
    fn scan_page(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
        view: ReadView,
    ) -> Result<Vec<(RowId, Row)>, ExecutionError> {
        let to_execution = |error: DbError| ExecutionError::DataSource(error.to_string());
        self.inner.health.check().map_err(to_execution)?;
        let include_history =
            view.snapshot_ts.0 < self.inner.vacuumed_through.load(Ordering::Acquire);
        let projections = self.inner.projections.read();
        let page = projections
            .scan_page(range, after, limit, view.snapshot_ts, include_history)
            .map_err(|error| to_execution(error.into()))?;
        let registry = projections.registry();
        Ok(page
            .into_iter()
            .map(|(row_id, row)| {
                (
                    row_id,
                    registry.adapt(row_id.entity_id(), row, view.schema_epoch),
                )
            })
            .collect())
    }
}

impl IndexAccess for Database {
    /// One page of an index scan; checks that the index exists, serves the snapshot and holds
    /// every requested field.
    fn index_scan_page(
        &self,
        index: IndexId,
        bounds: &IndexBounds,
        direction: ScanDirection,
        fields: &[FieldId],
        resume: Option<&[u8]>,
        limit: usize,
        view: ReadView,
    ) -> Result<IndexPage, ExecutionError> {
        let snapshot_ts = view.snapshot_ts;
        let to_execution = |error: DbError| ExecutionError::DataSource(error.to_string());
        self.inner.health.check().map_err(to_execution)?;
        let projections = self.inner.projections.read();
        let handle = projections.index(index).ok_or_else(|| {
            ExecutionError::DataSource(format!("index {} does not exist", index.0))
        })?;
        if !handle.serves(snapshot_ts) {
            return Err(ExecutionError::DataSource(format!(
                "{} cannot serve snapshot {} (it serves {} to {}); plan the query again",
                handle.name(),
                snapshot_ts.0,
                handle.usable_from().0,
                handle.watermark().0
            )));
        }
        let definition = handle.definition();
        if let Some(field) = fields.iter().find(|field| !definition.covers(**field)) {
            return Err(ExecutionError::InvalidPlan(format!(
                "index {} holds neither key nor covered field {field}",
                definition.name
            )));
        }
        // Visible entries are usually dense; the examined cap keeps a page bounded when a long
        // stretch of ended versions has to be skipped.
        let page = handle
            .projection
            .scan(
                bounds,
                snapshot_ts,
                direction,
                resume,
                limit,
                limit.saturating_mul(4).max(256),
            )
            .map_err(|error| to_execution(error.into()))?;
        let positions: Vec<(FieldId, Position)> = fields
            .iter()
            .map(|field| {
                let position = match definition.columns.iter().position(|c| c.field_id == *field) {
                    Some(at) => Position::Key(at),
                    None => Position::Included(
                        definition
                            .include
                            .iter()
                            .position(|f| f == field)
                            .expect("covered"),
                    ),
                };
                (*field, position)
            })
            .collect();
        let rows = page
            .entries
            .into_iter()
            .map(|entry| {
                let mut row = Row::new();
                for (field, position) in &positions {
                    let value = match position {
                        Position::Key(at) => &entry.key[*at],
                        Position::Included(at) => &entry.included[*at],
                    };
                    if !matches!(value, adb_core::Value::Null) {
                        row.fields.insert(*field, value.clone());
                    }
                }
                (entry.row_id, row)
            })
            .collect();
        Ok(IndexPage {
            rows,
            resume: page.resume,
            examined: page.examined,
        })
    }

    /// Rows at a snapshot, one descent per leaf of the primary index, adapted to the schemas of
    /// the view.
    fn fetch_rows(
        &self,
        row_ids: &[RowId],
        view: ReadView,
    ) -> Result<Vec<Option<Row>>, ExecutionError> {
        let to_execution = |error: DbError| ExecutionError::DataSource(error.to_string());
        self.inner.health.check().map_err(to_execution)?;
        let projections = self.inner.projections.read();
        let rows = projections
            .get_many_at(row_ids, view.snapshot_ts)
            .map_err(|error| to_execution(error.into()))?;
        let registry = projections.registry();
        Ok(row_ids
            .iter()
            .zip(rows)
            .map(|(row_id, row)| {
                row.map(|row| registry.adapt(row_id.entity_id(), row, view.schema_epoch))
            })
            .collect())
    }
}

/// Where a requested field sits in an index entry.
enum Position {
    /// Key column at this position.
    Key(usize),
    /// Covered column at this position.
    Included(usize),
}

/// Checks the foreign-key hints of `schema`: each names the primary key of a live entity (or of
/// `schema` itself) with the same type.
fn check_references(registry: &SchemaRegistry, schema: &EntitySchema) -> Result<(), DbError> {
    for field in schema.active_fields() {
        let Some(reference) = field.references else {
            continue;
        };
        let target = if reference.entity_id == schema.entity_id {
            Some(schema)
        } else {
            registry.latest(reference.entity_id).map(|t| &**t)
        };
        let Some(target) = target else {
            return Err(DbError::InvalidArgument(format!(
                "{} references unknown entity {}",
                field.name, reference.entity_id
            )));
        };
        let pk = target.primary_key_field();
        if reference.field_id != pk.id || pk.value_type != field.value_type {
            return Err(DbError::InvalidArgument(format!(
                "{} must reference the primary key of {} with the same type",
                field.name, target.name
            )));
        }
    }
    Ok(())
}
