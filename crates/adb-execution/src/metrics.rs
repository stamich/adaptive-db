//! Query counters.
/// Rows and batches produced by a cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryMetrics {
    /// Rows read from the data source (rows produced by the plan's leaves).
    pub source_rows: u64,
    /// Rows returned to the caller.
    pub output_rows: u64,
    /// Batches returned to the caller.
    pub batches: u64,
}

/// Counters of one execution that query telemetry records (Milestone 2.5.2.1).
///
/// A fixed-size value: reading it costs a few additions, unlike the per-operator profile, which is
/// a tree that is serialized to JSON. See [`crate::QueryCursor::telemetry`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryTelemetry {
    /// Rows returned to the caller.
    pub output_rows: u64,
    /// Rows read from storage: the rows produced by the plan's leaves.
    pub source_rows: u64,
    /// Batches returned to the caller.
    pub batches: u64,
    /// Highest number of bytes the query's blocking operators held at once.
    pub peak_memory_bytes: u64,
}
