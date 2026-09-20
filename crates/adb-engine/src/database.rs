//! Module `database` for crate `adb-engine`.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_core::{CommitTs, Row, RowId};
use adb_execution::{
    DataSource, ExecutionContext, ExecutionError, Executor, PhysicalPlan, QueryCursor,
};
use adb_storage::{
    Checkpoint, CheckpointStore, CurrentRecord, HistoricalVersion, IntegrityChecker,
    IntegrityReport, PersistentCurrentStore, PersistentVersionStore, StorageStats,
};
use adb_tx::{validate_write, Mutation, Transaction, TransactionManager};
use adb_wal::{SegmentedWalReader, SegmentedWalWriter, WalRecord, DEFAULT_SEGMENT_SIZE};
use parking_lot::Mutex;

use crate::{
    error::DbError,
    recovery::{analyze, collect_committed},
};

/// Defines the `WAL_DIR` constant used by this subsystem.
const WAL_DIR: &str = "wal";

/// Represents `Database` state used by this subsystem.
#[derive(Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

/// Represents `DatabaseInner` state used by this subsystem.
struct DatabaseInner {
    current: Mutex<PersistentCurrentStore>,
    versions: Mutex<PersistentVersionStore>,
    wal: Mutex<SegmentedWalWriter>,
    tx_manager: Mutex<TransactionManager>,
    commit_lock: Mutex<()>,
    checkpoint: CheckpointStore,
    wal_dir: PathBuf,
}

/// Implements behavior for `Database`.
impl Database {
    /// Implements the `open` operation used by this subsystem.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let dir = path.as_ref();
        fs::create_dir_all(dir)?;

        let wal_dir = dir.join(WAL_DIR);
        let entries = SegmentedWalReader::read_all(&wal_dir)?;
        let checkpoint = CheckpointStore::new(dir.join("checkpoint.meta"));
        let mut cp = checkpoint.load()?;

        let current = PersistentCurrentStore::open(dir.join("current"))?;
        let versions = PersistentVersionStore::open(dir.join("versions"))?;

        let committed = collect_committed(&entries)?;

        let mut newest_commit_lsn = cp.current_applied_lsn.max(cp.version_applied_lsn);

        for (commit_lsn, tx_id, commit_ts, pending) in committed {
            if commit_lsn.0 > cp.version_applied_lsn {
                for (row_id, version) in &pending.versions {
                    versions.put_at_lsn(*row_id, version, commit_lsn)?;
                }
            }

            if commit_lsn.0 > cp.current_applied_lsn {
                for (row_id, mutation) in &pending.mutations {
                    let existing = current.get(*row_id)?;

                    // Idempotent replay: do not replace a newer/equal
                    // durable current version.
                    if existing
                        .as_ref()
                        .is_some_and(|record| record.commit_ts >= commit_ts)
                    {
                        continue;
                    }

                    let record = match mutation {
                        Mutation::Put(row) => CurrentRecord {
                            commit_ts,
                            value: Some(row.clone()),
                        },
                        Mutation::Delete => CurrentRecord {
                            commit_ts,
                            value: None,
                        },
                    };

                    current.put_at_lsn(*row_id, &record, commit_lsn)?;
                }
            }

            newest_commit_lsn = newest_commit_lsn.max(commit_lsn.0);
            cp.last_commit_ts = cp.last_commit_ts.max(commit_ts.0);
            cp.last_tx_id = cp.last_tx_id.max(tx_id.0);
        }

        current.flush()?;
        versions.flush()?;

        cp.current_applied_lsn = newest_commit_lsn;
        cp.version_applied_lsn = newest_commit_lsn;

        let summary = analyze(entries.iter().map(|(_, record)| record.clone()));

        cp.last_commit_ts = cp.last_commit_ts.max(summary.max_commit_ts);
        cp.last_tx_id = cp.last_tx_id.max(summary.max_tx_id);
        checkpoint.save(cp)?;

        let mut tx_manager = TransactionManager::default();
        tx_manager.advance_after_recovery(cp.last_tx_id, cp.last_commit_ts);

        let wal = SegmentedWalWriter::open(&wal_dir, DEFAULT_SEGMENT_SIZE)?;

        Ok(Self {
            inner: Arc::new(DatabaseInner {
                current: Mutex::new(current),
                versions: Mutex::new(versions),
                wal: Mutex::new(wal),
                tx_manager: Mutex::new(tx_manager),
                commit_lock: Mutex::new(()),
                checkpoint,
                wal_dir,
            }),
        })
    }

    /// Implements the `begin` operation used by this subsystem.
    pub fn begin(&self) -> Transaction {
        self.inner.tx_manager.lock().begin()
    }

    /// Implements the `get` operation used by this subsystem.
    pub fn get(&self, row_id: RowId) -> Result<Option<Row>, DbError> {
        Ok(self
            .inner
            .current
            .lock()
            .get(row_id)?
            .and_then(|record| record.value))
    }

    /// Implements the `get_at` operation used by this subsystem.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Result<Option<Row>, DbError> {
        if let Some(current) = self.inner.current.lock().get(row_id)? {
            if current.commit_ts <= ts {
                return Ok(current.value);
            }
        }

        Ok(self
            .inner
            .versions
            .lock()
            .get_at(row_id, ts)?
            .and_then(|version| version.value))
    }

    /// Implements the `history` operation used by this subsystem.
    pub fn history(&self, row_id: RowId) -> Result<Vec<HistoricalVersion>, DbError> {
        let mut history = self.inner.versions.lock().history(row_id)?;

        if let Some(current) = self.inner.current.lock().get(row_id)? {
            // Current state is represented as an open-ended logical version
            // only in the history API. It is not persisted in VersionStore.
            history.push(HistoricalVersion {
                begin_ts: current.commit_ts,
                end_ts: CommitTs(u64::MAX),
                value: current.value,
            });
        }

        history.sort_by_key(|version| version.begin_ts);
        Ok(history)
    }

    /// Implements the `get_in_tx` operation used by this subsystem.
    pub fn get_in_tx(&self, tx: &Transaction, row_id: RowId) -> Result<Option<Row>, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }

        if let Some(local) = tx.local_read(row_id) {
            return Ok(local);
        }

        self.get_at(row_id, tx.snapshot_ts())
    }

    /// Implements the `commit` operation used by this subsystem.
    pub fn commit(&self, mut tx: Transaction) -> Result<CommitTs, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }

        let _commit_guard = self.inner.commit_lock.lock();

        // Validate and capture before-images while commit order is stable.
        let mut before_images = Vec::new();

        {
            let current = self.inner.current.lock();

            for row_id in tx.writes().keys() {
                let old = current.get(*row_id)?;

                if !validate_write(old.as_ref(), &tx) {
                    return Err(DbError::TransactionConflict);
                }

                if let Some(old) = old {
                    before_images.push((*row_id, old));
                }
            }
        }

        let commit_ts = self.inner.tx_manager.lock().allocate_commit_ts();

        let commit_lsn;

        {
            let mut wal = self.inner.wal.lock();

            wal.append(&WalRecord::Begin {
                tx_id: tx.id(),
                snapshot_ts: tx.snapshot_ts(),
            })?;

            // Persist before-images in WAL before the new values.
            for (row_id, old) in &before_images {
                wal.append(&WalRecord::Version {
                    tx_id: tx.id(),
                    row_id: *row_id,
                    begin_ts: old.commit_ts,
                    end_ts: commit_ts,
                    value: old.value.clone(),
                })?;
            }

            for (row_id, mutation) in tx.writes() {
                match mutation {
                    Mutation::Put(row) => {
                        wal.append(&WalRecord::Put {
                            tx_id: tx.id(),
                            row_id: *row_id,
                            value: row.clone(),
                        })?;
                    }

                    Mutation::Delete => {
                        wal.append(&WalRecord::Delete {
                            tx_id: tx.id(),
                            row_id: *row_id,
                        })?;
                    }
                }
            }

            commit_lsn = wal.append(&WalRecord::Commit {
                tx_id: tx.id(),
                commit_ts,
            })?;

            // Durability point: WAL before all data pages.
            wal.sync()?;
        }

        {
            let versions = self.inner.versions.lock();

            for (row_id, old) in &before_images {
                versions.put_at_lsn(
                    *row_id,
                    &HistoricalVersion {
                        begin_ts: old.commit_ts,
                        end_ts: commit_ts,
                        value: old.value.clone(),
                    },
                    commit_lsn,
                )?;
            }

            versions.flush()?;
        }

        {
            let current = self.inner.current.lock();

            for (row_id, mutation) in tx.writes() {
                let record = match mutation {
                    Mutation::Put(row) => CurrentRecord {
                        commit_ts,
                        value: Some(row.clone()),
                    },

                    Mutation::Delete => CurrentRecord {
                        commit_ts,
                        value: None,
                    },
                };

                current.put_at_lsn(*row_id, &record, commit_lsn)?;
            }

            current.flush()?;
        }

        // Publish checkpoint only after both stores are durable.
        self.inner.checkpoint.save(Checkpoint {
            format_version: adb_storage::CHECKPOINT_FORMAT_VERSION,
            current_applied_lsn: commit_lsn.0,
            version_applied_lsn: commit_lsn.0,
            last_commit_ts: commit_ts.0,
            last_tx_id: tx.id().0,
        })?;

        self.inner.tx_manager.lock().publish_commit(commit_ts);

        tx.mark_closed();
        Ok(commit_ts)
    }

    /// Implements the `rollback` operation used by this subsystem.
    pub fn rollback(&self, mut tx: Transaction) -> Result<(), DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }

        tx.mark_closed();
        Ok(())
    }

    /// Implements the `storage_stats` operation used by this subsystem.
    pub fn storage_stats(&self) -> Result<StorageStats, DbError> {
        let current = self.inner.current.lock();
        let versions = self.inner.versions.lock();

        Ok(StorageStats {
            current_rows: current.entries()?.len() as u64,
            historical_versions: versions.scan_all()?.len() as u64,
            current_heap_pages: current.heap_page_count()?,
            current_index_pages: current.index_page_count()?,
            version_heap_pages: versions.heap_page_count()?,
            version_index_pages: versions.index_page_count()?,
        })
    }

    /// Implements the `verify` operation used by this subsystem.
    pub fn verify(&self) -> Result<IntegrityReport, DbError> {
        let current = self.inner.current.lock();
        let versions = self.inner.versions.lock();

        Ok(IntegrityChecker::verify(&current, &versions)?)
    }

    /// Basic WAL retention for Milestone 1.6.
    /// Safe because Current and Version stores are independently persistent.
    pub fn prune_wal_before_checkpoint(&self) -> Result<usize, DbError> {
        let cp = self.inner.checkpoint.load()?;
        let removed = self
            .inner
            .wal
            .lock()
            .prune_segments_before(cp.replay_lsn())?;

        Ok(removed)
    }

    /// Implements the `wal_dir` operation used by this subsystem.
    pub fn wal_dir(&self) -> &Path {
        &self.inner.wal_dir
    }

    /// Implements the `latest_committed_ts` operation used by this subsystem.
    pub fn latest_committed_ts(&self) -> CommitTs {
        self.inner.tx_manager.lock().latest_committed_ts()
    }

    /// Implements the `execute` operation used by this subsystem.
    pub fn execute(&self, plan: PhysicalPlan) -> Result<QueryCursor, DbError> {
        let context = ExecutionContext::new(self.latest_committed_ts());
        Ok(Executor::execute(Arc::new(self.clone()), plan, context)?)
    }

    /// Implements the `execute_at` operation used by this subsystem.
    pub fn execute_at(
        &self,
        plan: PhysicalPlan,
        snapshot_ts: CommitTs,
    ) -> Result<QueryCursor, DbError> {
        Ok(Executor::execute(
            Arc::new(self.clone()),
            plan,
            ExecutionContext::new(snapshot_ts),
        )?)
    }
}

/// Implements behavior for `DataSource`.
impl DataSource for Database {
    /// Implements the `latest_committed_ts` operation used by this subsystem.
    fn latest_committed_ts(&self) -> CommitTs {
        Database::latest_committed_ts(self)
    }

    /// Implements the `point_lookup` operation used by this subsystem.
    fn point_lookup(
        &self,
        row_id: RowId,
        snapshot_ts: CommitTs,
    ) -> Result<Option<Row>, ExecutionError> {
        self.get_at(row_id, snapshot_ts)
            .map_err(|error| ExecutionError::DataSource(error.to_string()))
    }

    /// Implements the `scan_rows` operation used by this subsystem.
    fn scan_rows(&self, snapshot_ts: CommitTs) -> Result<Vec<(RowId, Row)>, ExecutionError> {
        let row_ids = self
            .inner
            .current
            .lock()
            .entries()
            .map_err(|error| ExecutionError::DataSource(error.to_string()))?
            .into_iter()
            .map(|(row_id, _)| row_id)
            .collect::<Vec<_>>();

        let mut rows = Vec::with_capacity(row_ids.len());
        for row_id in row_ids {
            if let Some(row) = self
                .get_at(row_id, snapshot_ts)
                .map_err(|error| ExecutionError::DataSource(error.to_string()))?
            {
                rows.push((row_id, row));
            }
        }

        Ok(rows)
    }
}
