//! Stable status codes of the C ABI.

/// Completion and error categories. Values are part of the ABI and never reused.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbStatus {
    /// Success.
    Ok = 0,
    /// Query cursor reached its end.
    EndOfStream = 1,
    /// Invalid pointer, length, cursor or serialized value.
    InvalidArgument = 2,
    /// Commit-time validation detected a conflict; retry the transaction.
    Conflict = 3,
    /// Filesystem failure.
    IoError = 4,
    /// Persistent bytes failed validation; projections can be rebuilt from the log.
    Corruption = 5,
    /// Query was cancelled.
    Cancelled = 6,
    /// Row or object not found.
    NotFound = 7,
    /// The instance is poisoned (or a commit's outcome is unknown); close and reopen it.
    Poisoned = 8,
    /// The requested change-log position is no longer retained.
    LogTruncated = 9,
    /// A query exceeded a resource limit (memory budget, materialized rows, join fanout or
    /// nested-loop comparisons); the database itself is unaffected.
    ResourceLimit = 10,
    /// An exact computation overflowed its result type (e.g. an INT64 SUM).
    ArithmeticOverflow = 11,
    /// Unexpected internal failure or contained panic.
    Internal = 255,
}
