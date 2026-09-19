//! Module `metrics` for crate `adb-execution`.
/// Represents `QueryMetrics` state used by this subsystem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryMetrics {
    pub source_rows: u64,
    pub output_rows: u64,
    pub batches: u64,
}
