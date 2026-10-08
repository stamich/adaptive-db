//! Errors of statistics collection and decoding.
use thiserror::Error;

/// Why statistics could not be collected or decoded.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StatsError {
    /// The `ANALYZE` options are malformed or outside their accepted ranges.
    #[error("invalid ANALYZE options: {0}")]
    InvalidOptions(String),
    /// Collection exceeded a configured bound (rows, fields, working set, deadline).
    #[error("ANALYZE limit exceeded: {0}")]
    Limit(String),
    /// The data source failed while scanning.
    #[error("data source error: {0}")]
    Source(String),
    /// A statistics document is malformed, oversized or of an unknown format.
    #[error("invalid statistics document: {0}")]
    Format(String),
}
