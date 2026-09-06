use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("WAL serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),

    #[error("corrupt WAL: {0}")]
    Corrupt(String),

    /// Serialized WAL record exceeds the hardened per-record limit.
    #[error("WAL record too large: {0} bytes")]
    RecordTooLarge(usize),
}
