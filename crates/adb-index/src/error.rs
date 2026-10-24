//! Errors of secondary indexes.
use adb_btree::BTreeError;
use thiserror::Error;

/// Failures of index definitions, maintenance and scans.
#[derive(Debug, Error)]
pub enum IndexError {
    /// A definition violates a rule or limit (caller error).
    #[error("invalid index definition: {0}")]
    InvalidDefinition(String),
    /// A row's encoded key or covered values exceed the index limits; the commit that would
    /// write it is rejected before it reaches the log.
    #[error("index {index}: a row's encoded key or covered values take {bytes} bytes, the limit is {limit}")]
    KeyTooLarge {
        /// Index name.
        index: String,
        /// Encoded size.
        bytes: usize,
        /// Limit exceeded.
        limit: usize,
    },
    /// Persisted index data failed validation; the index can be rebuilt from the log.
    #[error("corrupt index: {0}")]
    Corrupt(String),
    /// Failure of the underlying tree.
    #[error("index tree: {0}")]
    Tree(#[from] BTreeError),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl IndexError {
    /// Whether persisted bytes failed validation (rebuilding the index repairs it).
    pub fn is_corruption(&self) -> bool {
        match self {
            IndexError::Corrupt(_) => true,
            IndexError::Tree(error) => error.is_corruption(),
            _ => false,
        }
    }
}
