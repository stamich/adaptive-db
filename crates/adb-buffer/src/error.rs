//! Errors produced by the buffer pool and page store.
use adb_page::PageError;
use thiserror::Error;
/// Describes page-cache, page-file, and page-validation failures.
#[derive(Debug, Error)]
pub enum BufferError {
    /// Filesystem I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Page decoding/validation failed.
    #[error("page error: {0}")]
    Page(#[from] PageError),
    /// No frame can be evicted.
    #[error("buffer pool has no evictable frame")]
    NoEvictableFrame,
    /// Requested page is outside the complete page file.
    #[error("page {0} does not exist")]
    MissingPage(u64),
    /// Persistent page-file structure is invalid.
    #[error("corrupt page store: {0}")]
    CorruptStore(String),
}
