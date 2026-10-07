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
