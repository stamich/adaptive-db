//! Error module for the adb-storage crate.
//!
use thiserror::Error;

use adb_btree::BTreeError;
use adb_buffer::BufferError;
use adb_page::PageError;

/// Enumerates the supported `StorageError` variants used by this subsystem.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("buffer error: {0}")]
    Buffer(#[from] BufferError),
    #[error("btree error: {0}")]
    BTree(#[from] BTreeError),
    #[error("page error: {0}")]
    Page(#[from] PageError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
    /// Persistent storage metadata is structurally invalid or checksum-corrupt.
    #[error("corrupt storage: {0}")]
    Corrupt(String),
}
