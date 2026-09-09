//! Database module for the adb-engine crate.
//!
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_core::{CommitTs, Lsn, Row, RowId, TxId};
use adb_storage::{
    Checkpoint, CheckpointStore, CurrentRecord, HistoricalVersion, PersistentCurrentStore,
    VersionStore,
};
use adb_tx::{Mutation, Transaction, TransactionManager, validate_write};
use adb_wal::{WalReader, WalRecord, WalWriter};
use parking_lot::{Mutex, RwLock};

use crate::{error::DbError, recovery::recover};

/// Defines the `WAL_FILE` constant used by this subsystem.
const WAL_FILE: &str = "wal.log";

/// Represents an Adaptive DB database handle and coordinates transactions, WAL, storage, and recovery.
pub struct Database {
    inner: Arc<DatabaseInner>,
}

/// Represents `DatabaseInner` state used by the src subsystem.
struct DatabaseInner {
    current: Mutex<PersistentCurrentStore>,
    versions: RwLock<VersionStore>,
    wal: Mutex<WalWriter>,
    tx_manager: Mutex<TransactionManager>,
    commit_lock: Mutex<()>,
    checkpoint: CheckpointStore,
    wal_path: PathBuf,
}

/// Implements behavior for `Database`.
impl Database {
    /// Opens or creates the underlying resource and reconstructs the runtime state required by this subsystem.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let dir = path.as_ref();
        fs::create_dir_all(dir)?;
        let wal_path = dir.join(WAL_FILE);
        let entries = if wal_path.exists() {
            WalReader::open(&wal_path)?.read_all()?
        } else {
            Vec::new()
        };
        let logical = recover(entries.iter().map(|(_, r)| r.clone()));

        let checkpoint = CheckpointStore::new(dir.join("checkpoint.meta"));
        let cp = checkpoint.load()?;
        let current = PersistentCurrentStore::open(dir.join("current"))?;
        replay_current_tail(&entries, cp.last_applied_commit_lsn, &current)?;
        current.flush()?;

        let latest_commit_lsn = entries
            .iter()
            .rev()
            .find_map(|(lsn, r)| matches!(r, WalRecord::Commit { .. }).then_some(lsn.0))
            .unwrap_or(cp.last_applied_commit_lsn);
        if logical.max_commit_ts > cp.last_commit_ts
            || latest_commit_lsn > cp.last_applied_commit_lsn
        {
            checkpoint.save(Checkpoint {
                last_applied_commit_lsn: latest_commit_lsn,
                last_commit_ts: logical.max_commit_ts,
            })?;
        }

        let mut tx_manager = TransactionManager::default();
        tx_manager.advance_after_recovery(logical.max_tx_id, logical.max_commit_ts);
        let wal = WalWriter::open(&wal_path)?;

        Ok(Self {
            inner: Arc::new(DatabaseInner {
                current: Mutex::new(current),
                versions: RwLock::new(logical.stores.versions),
                wal: Mutex::new(wal),
                tx_manager: Mutex::new(tx_manager),
                commit_lock: Mutex::new(()),
                checkpoint,
                wal_path,
            }),
        })
    }

    /// Starts a new transaction at the latest published MVCC snapshot.
    pub fn begin(&self) -> Transaction {
        self.inner.tx_manager.lock().begin()
    }

    /// Returns the value visible for the requested key or row at the operation's default snapshot.
    pub fn get(&self, row_id: RowId) -> Result<Option<Row>, DbError> {
        Ok(self.inner.current.lock().get(row_id)?.and_then(|r| r.value))
    }

    /// Returns the row version visible at the supplied historical commit timestamp.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Result<Option<Row>, DbError> {
        if let Some(current) = self.inner.current.lock().get(row_id)? {
            if current.commit_ts <= ts {
                return Ok(current.value);
            }
        }
        Ok(self
            .inner
            .versions
            .read()
            .get_at(row_id, ts)
            .and_then(|v| v.value.clone()))
    }

    /// Returns the row visible inside the supplied transaction, including transaction-local writes.
    pub fn get_in_tx(&self, tx: &Transaction, row_id: RowId) -> Result<Option<Row>, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }
        if let Some(local) = tx.local_read(row_id) {
            return Ok(local);
        }
        self.get_at(row_id, tx.snapshot_ts())
    }

    /// Validates and durably commits a transaction before publishing its commit timestamp.
    pub fn commit(&self, mut tx: Transaction) -> Result<CommitTs, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }
        let _guard = self.inner.commit_lock.lock();

        {
            let current = self.inner.current.lock();
            for row_id in tx.writes().keys() {
                let record = current.get(*row_id)?;
                if !validate_write(record.as_ref(), &tx) {
                    return Err(DbError::TransactionConflict);
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
            wal.sync()?;
        }

        {
            let current = self.inner.current.lock();
            let mut versions = self.inner.versions.write();
            for (row_id, mutation) in tx.writes() {
                if let Some(old) = current.get(*row_id)? {
                    versions.push(
                        *row_id,
                        HistoricalVersion {
                            begin_ts: old.commit_ts,
                            end_ts: commit_ts,
                            value: old.value,
                        },
                    );
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
            current.flush()?;
        }

        self.inner.checkpoint.save(Checkpoint {
            last_applied_commit_lsn: commit_lsn.0,
            last_commit_ts: commit_ts.0,
        })?;
        self.inner.tx_manager.lock().publish_commit(commit_ts);
        tx.mark_closed();
        Ok(commit_ts)
    }

    /// Closes a transaction without applying its local mutations.
    pub fn rollback(&self, mut tx: Transaction) -> Result<(), DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }
        tx.mark_closed();
        Ok(())
    }

    /// Implements the `wal_path` operation used by this subsystem.
    pub fn wal_path(&self) -> &Path {
        &self.inner.wal_path
    }
}

/// Implements the `replay_current_tail` operation used by this subsystem.
fn replay_current_tail(
    entries: &[(Lsn, WalRecord)],
    checkpoint_lsn: u64,
    current: &PersistentCurrentStore,
) -> Result<(), DbError> {
    /// Represents `Pending` state used by the src subsystem.
    #[derive(Default)]
    struct Pending {
        mutations: Vec<(RowId, Mutation)>,
    }
    let mut pending: HashMap<TxId, Pending> = HashMap::new();

    for (lsn, record) in entries {
        match record {
            WalRecord::Begin { tx_id, .. } => {
                pending.entry(*tx_id).or_default();
            }
            WalRecord::Put {
                tx_id,
                row_id,
                value,
            } => pending
                .entry(*tx_id)
                .or_default()
                .mutations
                .push((*row_id, Mutation::Put(value.clone()))),
            WalRecord::Delete { tx_id, row_id } => pending
                .entry(*tx_id)
                .or_default()
                .mutations
                .push((*row_id, Mutation::Delete)),
            WalRecord::Abort { tx_id } => {
                pending.remove(tx_id);
            }
            WalRecord::Commit { tx_id, commit_ts } => {
                let tx = pending.remove(tx_id).unwrap_or_default();
                if lsn.0 > checkpoint_lsn {
                    for (row_id, mutation) in tx.mutations {
                        let record = match mutation {
                            Mutation::Put(row) => CurrentRecord {
                                commit_ts: *commit_ts,
                                value: Some(row),
                            },
                            Mutation::Delete => CurrentRecord {
                                commit_ts: *commit_ts,
                                value: None,
                            },
                        };
                        current.put_at_lsn(row_id, &record, *lsn)?;
                    }
                }
            }
        }
    }
    Ok(())
}
