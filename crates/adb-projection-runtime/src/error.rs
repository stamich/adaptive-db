//! Errors of the projection runtime.
use thiserror::Error;

/// Everything that can go wrong while registering, advancing or waiting for projections.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProjectionError {
    /// A descriptor or argument is not acceptable.
    #[error("invalid projection argument: {0}")]
    Invalid(String),
}
