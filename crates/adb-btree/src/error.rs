//! B+Tree errors.
use thiserror::Error;

use adb_buffer::BufferError;
use adb_journal::JournalError;

/// Failures of B+Tree operations.
#[derive(Debug, Error)]
pub enum BTreeError {
    /// Page cache or page file failure.
    #[error("buffer error: {0}")]
    Buffer(#[from] BufferError),
    /// Metadata publication failure.
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Metadata (de)serialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
    /// Structural corruption (bad node, cycle, excessive depth, bad metadata).
    #[error("corrupt btree: {0}")]
    Corrupt(String),
}

impl BTreeError {
    /// Whether persisted bytes failed validation.
    pub fn is_corruption(&self) -> bool {
        match self {
            BTreeError::Corrupt(_) | BTreeError::Serialization(_) => true,
            BTreeError::Buffer(error) => error.is_corruption(),
            BTreeError::Journal(adb_journal::JournalError::Corrupt(_)) => true,
            _ => false,
        }
    }
}
