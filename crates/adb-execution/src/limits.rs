//! Hardened resource limits of the execution layer.
//!
//! Structural limits are constants checked once by plan validation.

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
