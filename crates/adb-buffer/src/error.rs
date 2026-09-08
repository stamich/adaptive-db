//! Error module for the adb-buffer crate.
//!
use thiserror::Error;

use adb_page::PageError;

/// Enumerates the supported `BufferError` variants used by this subsystem.
#[derive(Debug, Error)]
pub enum BufferError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("page error: {0}")]
    Page(#[from] PageError),
    #[error("buffer pool has no evictable frame")]
    NoEvictableFrame,
    #[error("page {0} does not exist")]
    MissingPage(u64),
    /// Persistent page-file structure is inconsistent.
    #[error("corrupt page store: {0}")]
    CorruptStore(String),
}
