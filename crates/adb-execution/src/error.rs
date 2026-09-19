//! Module `error` for crate `adb-execution`.
use thiserror::Error;

/// Enumerates `ExecutionError` alternatives used by this subsystem.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("data source error: {0}")]
    DataSource(String),

    #[error("invalid physical plan: {0}")]
    InvalidPlan(String),

    #[error("expression error: {0}")]
    Expression(String),

    #[error("inconsistent physical type for field {0}")]
    InconsistentType(u32),

    #[error("query cancelled")]
    Cancelled,

    #[error("query deadline exceeded")]
    DeadlineExceeded,

    #[error("wire format error: {0}")]
    Wire(String),

    /// Query execution exceeded a configured in-memory resource boundary.
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),
}
