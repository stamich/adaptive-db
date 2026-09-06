use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_core::{CommitTs, Row, RowId};
use adb_storage::Stores;
use adb_tx::{validate_write, Mutation, Transaction, TransactionManager};
use adb_wal::{WalReader, WalRecord, WalWriter};
use parking_lot::{Mutex, RwLock};

use crate::{
    error::DbError,
    recovery::{recover, RecoveryResult},
};

const WAL_FILE: &str = "wal.log";

pub struct Database {
    inner: Arc<DatabaseInner>,
}

struct DatabaseInner {
    stores: RwLock<Stores>,
    wal: Mutex<WalWriter>,
    tx_manager: Mutex<TransactionManager>,
    commit_lock: Mutex<()>,
    wal_path: PathBuf,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let dir = path.as_ref();
        fs::create_dir_all(dir)?;

        let wal_path = dir.join(WAL_FILE);

        let recovery_result = if wal_path.exists() {
            let records = WalReader::open(&wal_path)?
                .read_all()?
                .into_iter()
                .map(|(_, record)| record)
                .collect::<Vec<_>>();

            recover(records)
        } else {
            RecoveryResult::default()
        };

        let mut tx_manager = TransactionManager::default();
        tx_manager.advance_after_recovery(
            recovery_result.max_tx_id,
            recovery_result.max_commit_ts,
        );

        let wal = WalWriter::open(&wal_path)?;

        Ok(Self {
            inner: Arc::new(DatabaseInner {
                stores: RwLock::new(recovery_result.stores),
                wal: Mutex::new(wal),
                tx_manager: Mutex::new(tx_manager),
                commit_lock: Mutex::new(()),
                wal_path,
            }),
        })
    }

    pub fn begin(&self) -> Transaction {
        self.inner.tx_manager.lock().begin()
    }

    pub fn get(&self, row_id: RowId) -> Option<Row> {
        let ts = self.inner.tx_manager.lock().latest_committed_ts();
        self.inner.stores.read().read_at(row_id, ts)
    }

    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Option<Row> {
        self.inner.stores.read().read_at(row_id, ts)
    }

    pub fn get_in_tx(
        &self,
        tx: &Transaction,
        row_id: RowId,
    ) -> Result<Option<Row>, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }

        if let Some(local) = tx.local_read(row_id) {
            return Ok(local);
        }

        Ok(self
            .inner
            .stores
            .read()
            .read_at(row_id, tx.snapshot_ts()))
    }

    pub fn commit(&self, mut tx: Transaction) -> Result<CommitTs, DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }

        let _commit_guard = self.inner.commit_lock.lock();

        {
            let stores = self.inner.stores.read();

            for row_id in tx.writes().keys() {
                let current = stores.current.get(*row_id);

                if !validate_write(current, &tx) {
                    return Err(DbError::TransactionConflict);
                }
            }
        }

        let commit_ts = self.inner.tx_manager.lock().allocate_commit_ts();

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

            wal.append(&WalRecord::Commit {
                tx_id: tx.id(),
                commit_ts,
            })?;

            // Durability point.
            wal.sync()?;
        }

        {
            let mut stores = self.inner.stores.write();

            for (row_id, mutation) in tx.writes() {
                match mutation {
                    Mutation::Put(row) => {
                        stores.apply_put(*row_id, row.clone(), commit_ts);
                    }

                    Mutation::Delete => {
                        stores.apply_delete(*row_id, commit_ts);
                    }
                }
            }
        }

        // Only now may new transactions observe this commit timestamp.
        self.inner
            .tx_manager
            .lock()
            .publish_commit(commit_ts);

        tx.mark_closed();

        Ok(commit_ts)
    }

    pub fn rollback(&self, mut tx: Transaction) -> Result<(), DbError> {
        if tx.is_closed() {
            return Err(DbError::TransactionClosed);
        }

        // No global state was changed, so rollback is intentionally cheap.
        tx.mark_closed();
        Ok(())
    }

    pub fn wal_path(&self) -> &Path {
        &self.inner.wal_path
    }
}
