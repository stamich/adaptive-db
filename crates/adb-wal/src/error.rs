//! Errors returned by WAL readers and writers.
use std::io;
use thiserror::Error;
/// Describes durable WAL I/O, serialization, validation, and configuration failures.
#[derive(Debug, Error)]
pub enum WalError {
    /// Underlying filesystem I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// WAL record serialization or deserialization failed.
    #[error("WAL serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
    /// A persisted WAL frame or segment is structurally corrupt.
    #[error("corrupt WAL: {0}")]
    Corrupt(String),
    /// A serialized WAL record exceeds the hardened frame limit.
    #[error("WAL record too large: {0} bytes")]
    RecordTooLarge(usize),
    /// Segmented WAL configuration cannot be represented by the LSN encoding.
    #[error("invalid WAL configuration: {0}")]
    InvalidConfiguration(String),
}
