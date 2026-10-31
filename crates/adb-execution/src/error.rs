//! Query execution errors.
use thiserror::Error;

/// Why a query could not produce its next batch.
#[derive(Debug, Error)]
pub enum ExecutionError {
    /// The data source failed (storage error, poisoned engine).
    #[error("data source error: {0}")]
    DataSource(String),

    /// The plan violates a structural or size limit.
    #[error("invalid physical plan: {0}")]
    InvalidPlan(String),

    /// An expression could not be evaluated (e.g. incompatible types).
    #[error("expression error: {0}")]
    Expression(String),

    /// Rows of one batch disagree on the physical type of this field.
    #[error("inconsistent physical type for field {0}")]
    InconsistentType(u32),

    /// The query was cancelled.
    #[error("query cancelled")]
    Cancelled,

    /// The query ran past its deadline.
    #[error("query deadline exceeded")]
    DeadlineExceeded,

    /// A batch could not be encoded in the wire format.
    #[error("wire format error: {0}")]
    Wire(String),

    /// An exact computation (e.g. an INT64 SUM) does not fit its result type.
    #[error("arithmetic overflow: {0}")]
    ArithmeticOverflow(String),

    /// Query execution exceeded a configured in-memory resource boundary.
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),

    /// The plan was made at another catalog epoch (Milestone 2.4.0): a table or index it was
    /// planned against may have changed. Safe to plan again and retry.
    #[error(
        "the plan was made at schema epoch {planned}, the catalog is at epoch {current}; plan \
         the query again"
    )]
    SchemaChanged {
        /// Epoch the plan was made at.
        planned: u64,
        /// Current epoch.
        current: u64,
    },
}
