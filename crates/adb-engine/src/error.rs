use thiserror::Error;

use adb_wal::WalError;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("WAL error: {0}")]
    Wal(#[from] WalError),

    #[error("transaction conflict")]
    TransactionConflict,

    #[error("transaction already closed")]
    TransactionClosed,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
