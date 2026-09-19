//! Module `error` for crate `adb-core`.
use thiserror::Error;

/// Enumerates `CoreError` alternatives used by this subsystem.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid data: {0}")]
    InvalidData(String),
}
