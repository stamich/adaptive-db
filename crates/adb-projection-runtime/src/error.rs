//! Errors of the projection runtime.
use thiserror::Error;

/// Everything that can go wrong while registering, advancing or waiting for projections.
#[derive(Debug, Error)]
pub enum ProjectionError {
    /// A descriptor or argument is not acceptable.
    #[error("invalid projection argument: {0}")]
    Invalid(String),
    /// A file operation failed.
    #[error("projection checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The checkpoint file is damaged or was written by an incompatible version.
    #[error("projection checkpoint is corrupt: {0}")]
    Corrupt(String),
}
