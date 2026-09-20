//! Module `error` for crate `adb-engine`.
use thiserror::Error;

use adb_execution::ExecutionError;
use adb_storage::StorageError;
use adb_wal::WalError;

/// Enumerates `DbError` alternatives used by this subsystem.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("WAL error: {0}")]
    Wal(#[from] WalError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("execution error: {0}")]
    Execution(#[from] ExecutionError),

    #[error("transaction conflict")]
    TransactionConflict,

    #[error("transaction already closed")]
    TransactionClosed,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
