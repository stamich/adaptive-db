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
}
