//! Errors of the change log.
use thiserror::Error;

/// Everything that can go wrong while encoding, storing or reading change-log data.
#[derive(Debug, Error)]
pub enum ChangeLogError {
    /// An envelope is larger than [`crate::MAX_ENVELOPE_BYTES`] (when encoding) or claims to be
    /// (when decoding), so it is refused instead of allocated.
    #[error("change envelope of {size} bytes exceeds the limit of {limit} bytes")]
    TooLarge {
        /// Size of the offending envelope.
        size: u64,
        /// Configured limit.
        limit: u64,
    },
    /// Bytes could not be decoded into an envelope.
    #[error("malformed change envelope: {0}")]
    Malformed(String),
}
