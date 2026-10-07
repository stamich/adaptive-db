//! Query counters.
/// Rows and batches produced by a cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryMetrics {
    pub source_rows: u64,
    pub output_rows: u64,
    pub batches: u64,
}
