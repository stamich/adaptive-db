//! Error module for the adb-core crate.
//!
use thiserror::Error;

/// Enumerates the supported `CoreError` variants used by this subsystem.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid data: {0}")]
    InvalidData(String),
}
