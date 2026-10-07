//! Errors produced by the buffer pool and page store.
use adb_page::PageError;
use thiserror::Error;

/// Page-cache, page-file and page-validation failures.
#[derive(Debug, Error)]
pub enum BufferError {
    /// Filesystem I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Page decoding/validation failed.
    #[error("page error: {0}")]
    Page(#[from] PageError),
    /// Requested page was never allocated.
    #[error("page {0} does not exist")]
    MissingPage(u64),
    /// Persistent page-file structure is invalid.
    #[error("corrupt page store: {0}")]
    CorruptStore(String),
}

impl BufferError {
    /// Whether persisted bytes failed validation (as opposed to an I/O or usage error).
    pub fn is_corruption(&self) -> bool {
        matches!(self, BufferError::Page(_) | BufferError::CorruptStore(_))
    }
}
