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

use adb_core::{CommitTs, KeyRange, Row, RowId};
use adb_execution::{
    DataSource, ExecutionContext, ExecutionError, Executor, PhysicalPlan, QueryCursor,
};
use adb_journal::{Journal, JOURNAL_FILE};
use adb_stats::{AnalyzeOptions, TableStatistics};
use adb_storage::{
    Checkpoint, CheckpointStore, Checkpointable, HistoricalVersion, IntegrityReport, StorageStats,
};
use adb_tx::{validate, IsolationLevel, Transaction, TransactionManager};
use adb_wal::earliest_lsn;
use parking_lot::{Mutex, RwLock};

use crate::{
    cdc::{self, ChangeBatch, ChangeCursor, ChangeFilter},
    committed::CommittedTx,
    health::EngineHealth,
    log::LogWriter,
    offsets::ConsumerOffsets,
    projections::{Projections, CURRENT_DIR, VERSIONS_DIR},
    recovery::{recover, write_checkpoint},
    statistics::{modifications_path, ModificationCounters, StatisticsStore},
    DatabaseOptions, DbError,
};

/// Log directory inside the database directory.
const WAL_DIR: &str = "wal";
/// Checkpoint record file inside the database directory.
const CHECKPOINT_FILE: &str = "checkpoint.meta";

/// Result of [`Database::vacuum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VacuumReport {
    /// Tombstones removed from the current store.
    pub tombstones_removed: u64,
    /// Tombstones committed at or before this timestamp were eligible.
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
    /// kept (they describe a snapshot, not the store).
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
        for path in [
            dir.join(CHECKPOINT_FILE),
            dir.join(JOURNAL_FILE),
            dir.join("checkpoint.journal.tmp"),
            modifications_path(dir),
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

    /// Reads a row inside `tx`: its own writes first, then its snapshot. The read is recorded
    /// for serializable validation.
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

    /// State of a row at snapshot `ts`.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Result<Option<Row>, DbError> {
        self.inner.health.check()?;
        Ok(self.inner.projections.read().get_at(row_id, ts)?)
    }

    /// Every version of a row, oldest first; the current one has an open end.
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
        Ok(Executor::execute(
            Arc::new(self.clone()),
            plan,
            ExecutionContext::new(snapshot_ts),
        )?)
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
    /// the counter is a staleness signal, not an exact delta.
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
        self.inner.statistics.save(statistics.clone())?;
        Ok(statistics)
    }

    /// The statistics document of `entity_id`, if it was analyzed.
    pub fn statistics(&self, entity_id: u64) -> Option<TableStatistics> {
        self.inner.statistics.get(entity_id)
    }

    /// Row mutations committed to `entity_id` since its last `ANALYZE` (since creation if it
    /// was never analyzed).
    pub fn modifications_since_analyze(&self, entity_id: u64) -> u64 {
        let total = self.inner.modifications.lock().get(entity_id);
        let at_analyze = self
            .inner
            .statistics
            .get(entity_id)
            .map_or(0, |statistics| statistics.modifications_at_analyze);
        total.saturating_sub(at_analyze)
    }

    // ----- maintenance ----------------------------------------------------------------------

    /// Persists all in-memory projection changes and moves the recovery start point forward.
    pub fn checkpoint(&self) -> Result<(), DbError> {
        self.inner.health.check()?;
        let _commit = self.inner.commit_lock.lock();
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
        let removed = self
            .inner
            .health
            .guard(|| {
                let lsn = self.inner.log.end()?;
                Ok(self.inner.projections.write().vacuum(horizon, lsn)?)
            })
            .map_err(DbError::Poisoned)?;
        Ok(VacuumReport {
            tombstones_removed: removed,
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

    /// Temporal integrity check of both projections.
    pub fn verify(&self) -> Result<IntegrityReport, DbError> {
        Ok(self.inner.projections.read().verify()?)
    }

    /// Directory of the canonical log.
    pub fn wal_dir(&self) -> &Path {
        &self.inner.wal_dir
    }

    // ----- internals ------------------------------------------------------------------------

    /// Validates `tx` and builds what its commit will write. Caller holds the commit lock.
    fn prepare(&self, tx: &Transaction) -> Result<CommittedTx, DbError> {
        let projections = self.inner.projections.read();
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
            mutations: tx
                .writes()
                .iter()
                .map(|(row_id, mutation)| (*row_id, mutation.clone()))
                .collect(),
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

    /// Snapshot read of one row for `PointLookup`.
    fn point_lookup(
        &self,
        row_id: RowId,
        snapshot_ts: CommitTs,
    ) -> Result<Option<Row>, ExecutionError> {
        self.get_at(row_id, snapshot_ts)
            .map_err(|error| ExecutionError::DataSource(error.to_string()))
    }

    /// Page of a range scan; merges the version index when the snapshot predates the vacuum horizon.
    fn scan_page(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
        snapshot_ts: CommitTs,
    ) -> Result<Vec<(RowId, Row)>, ExecutionError> {
        let to_execution = |error: DbError| ExecutionError::DataSource(error.to_string());
        self.inner.health.check().map_err(to_execution)?;
        let include_history = snapshot_ts.0 < self.inner.vacuumed_through.load(Ordering::Acquire);
        self.inner
            .projections
            .read()
            .scan_page(range, after, limit, snapshot_ts, include_history)
            .map_err(|error| to_execution(error.into()))
    }
}
