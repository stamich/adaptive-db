//! Errors raised while validating core value types.
use thiserror::Error;

/// Invalid core data.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A value violated a documented invariant.
    #[error("invalid data: {0}")]
    InvalidData(String),
}
