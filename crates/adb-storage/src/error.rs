//! Storage errors.
use thiserror::Error;

use adb_btree::BTreeError;
use adb_buffer::BufferError;
use adb_journal::JournalError;
use adb_page::PageError;

/// Failures of the storage projections.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Page cache failure.
    #[error("buffer error: {0}")]
    Buffer(#[from] BufferError),
    /// Index failure.
    #[error("btree error: {0}")]
    BTree(#[from] BTreeError),
    /// Heap page failure.
    #[error("page error: {0}")]
    Page(#[from] PageError),
    /// Metadata publication failure.
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Record (de)serialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
    /// Persisted state violates an invariant.
    #[error("invalid storage state: {0}")]
    Invalid(String),
}

impl StorageError {
    /// Whether persisted bytes failed validation.
    pub fn is_corruption(&self) -> bool {
        match self {
            StorageError::Invalid(_) | StorageError::Page(_) | StorageError::Serialization(_) => {
                true
            }
            StorageError::Buffer(error) => error.is_corruption(),
            StorageError::BTree(error) => error.is_corruption(),
            StorageError::Journal(JournalError::Corrupt(_)) => true,
            _ => false,
        }
    }
}
