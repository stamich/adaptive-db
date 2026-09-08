//! Error module for the adb-btree crate.
//!
use thiserror::Error;

use adb_buffer::BufferError;

/// Enumerates the supported `BTreeError` variants used by this subsystem.
#[derive(Debug, Error)]
pub enum BTreeError {
    #[error("buffer error: {0}")]
    Buffer(#[from] BufferError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
    #[error("corrupt btree: {0}")]
    Corrupt(String),
}
