//! Stable status codes returned across the Milestone 2 C ABI.

/// Enumerates C ABI completion and error categories.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbStatus {
    /// Operation completed successfully.
    Ok = 0,
    /// Query cursor reached end of stream.
    EndOfStream = 1,
    /// Caller supplied an invalid pointer, length or serialized value.
    InvalidArgument = 2,
    /// Optimistic transaction validation detected a conflict.
    Conflict = 3,
    /// Filesystem or I/O operation failed.
    IoError = 4,
    /// Durable engine bytes or metadata were detected as corrupt.
    Corruption = 5,
    /// Query execution was cooperatively cancelled.
    Cancelled = 6,
    /// Requested row or object was not found.
    NotFound = 7,
    /// Unexpected internal failure or contained panic.
    Internal = 255,
}
