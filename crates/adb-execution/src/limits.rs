//! Hardened resource limits of the execution layer.
//!
//! Structural limits are constants checked once by plan validation; runtime limits live in
//! [`ExecutionLimits`] so that callers (and tests) can tighten them per query.

/// Deepest operator tree accepted.
pub const MAX_PLAN_DEPTH: usize = 128;
/// Deepest expression tree accepted.
pub const MAX_EXPR_DEPTH: usize = 128;
/// Longest list accepted in one plan node (scan columns, projected slots, keys, aggregates).
pub const MAX_LIST_LEN: usize = 4096;
/// Slots per query; slot ids must be below this value.
pub const MAX_SLOTS: u32 = 4096;
/// Largest `LIMIT` / `TopK` row count accepted.
pub const MAX_LIMIT: usize = 1_000_000;

/// Runtime limits enforced while a query runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// Total bytes a query may hold in blocking operators (hash tables, sort buffers, groups).
    pub query_memory_bytes: usize,
    /// Rows a blocking operator may materialize (join build side, sort input, groups).
    pub max_materialized_rows: usize,
    /// Matches a single left row may produce in a join.
    pub max_join_fanout: usize,
    /// Predicate evaluations a nested-loop join may perform in total.
    pub max_nested_loop_comparisons: u64,
}

impl Default for ExecutionLimits {
    /// The hardened 2.1 defaults.
    fn default() -> Self {
        Self {
            query_memory_bytes: 256 * 1024 * 1024,
            max_materialized_rows: 1_000_000,
            max_join_fanout: 16_384,
            max_nested_loop_comparisons: 10_000_000,
        }
    }
}
