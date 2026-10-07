//! Top-level engine errors.
use adb_core::Lsn;
use adb_execution::ExecutionError;
use adb_journal::JournalError;
use adb_storage::StorageError;
use adb_tx::Conflict;
use adb_wal::WalError;
use thiserror::Error;

/// Errors returned by [`crate::Database`].
#[derive(Debug, Error)]
pub enum DbError {
    /// Canonical-log failure.
    #[error("WAL error: {0}")]
    Wal(#[from] WalError),
    /// Projection (store) failure.
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    /// Checkpoint-journal failure.
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    /// Query execution failure.
    #[error("execution error: {0}")]
    Execution(#[from] ExecutionError),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Commit-time validation failed; the transaction had no effect and may be retried.
    #[error("transaction conflict: {0:?}")]
    TransactionConflict(Conflict),
    /// The transaction was already committed or rolled back.
    #[error("transaction already closed")]
    TransactionClosed,
    /// A failure after the commit started writing to the log: the transaction may or may not
    /// be durable. The database is now poisoned and must be reopened; recovery decides.
    #[error("commit outcome unknown, database must be reopened: {0}")]
    CommitOutcomeUnknown(String),
    /// An earlier failure left in-memory state that may diverge from the log.
    #[error("database is poisoned and must be reopened: {0}")]
    Poisoned(String),
    /// The requested log position is no longer retained.
    #[error("change log position {requested:?} is no longer retained (earliest is {earliest:?})")]
    ChangeLogTruncated {
        /// Requested position.
        requested: Lsn,
        /// Oldest retained position.
        earliest: Lsn,
    },
    /// Caller-supplied argument is invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl DbError {
    /// Whether the error indicates corrupted persistent state (a projection can then be rebuilt
    /// from the log with `Database::rebuild_projections`).
    pub fn is_corruption(&self) -> bool {
        match self {
            DbError::Wal(WalError::Corrupt(_) | WalError::Serialization(_)) => true,
            DbError::Journal(JournalError::Corrupt(_)) => true,
            DbError::Storage(error) => error.is_corruption(),
            _ => false,
        }
    }
}
