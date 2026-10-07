//! Errors returned by the log writer and readers.
use std::io;

use adb_core::Lsn;
use thiserror::Error;

/// Log I/O, framing and configuration failures.
#[derive(Debug, Error)]
pub enum WalError {
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Record (de)serialization failure.
    #[error("WAL serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
    /// A frame or segment is structurally corrupt.
    #[error("corrupt WAL: {0}")]
    Corrupt(String),
    /// A record exceeds the frame limit.
    #[error("WAL record too large: {0} bytes")]
    RecordTooLarge(usize),
    /// The configuration cannot be represented by the LSN encoding.
    #[error("invalid WAL configuration: {0}")]
    InvalidConfiguration(String),
    /// The requested position is older than the oldest retained segment.
    #[error("log position {requested:?} is no longer retained (earliest is {earliest:?})")]
    Truncated {
        /// Requested position.
        requested: Lsn,
        /// Oldest readable position.
        earliest: Lsn,
    },
}
