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
    /// A file operation failed.
    #[error("change log I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A segment holds damage that is not a torn tail: data after the damage, a wrong header or
    /// out-of-order sequence numbers. The log is not guessed at; the caller rebuilds it.
    #[error("change log segment {segment} is corrupt at offset {offset}: {reason}")]
    Corrupt {
        /// File name of the damaged segment.
        segment: String,
        /// Byte offset of the damaged frame.
        offset: u64,
        /// What is wrong.
        reason: String,
    },
    /// An append would not advance the log: the sequence number is not above the head.
    #[error("commit sequence {got} does not follow the change log head {head}")]
    Regression {
        /// Current head.
        head: u64,
        /// Offered sequence number.
        got: u64,
    },
    /// An envelope with this sequence number is stored, but with different content. The caller
    /// holds a diverged copy of history; nothing is overwritten.
    #[error("commit sequence {seq} is already in the change log with different content")]
    Mismatch {
        /// The contested sequence number.
        seq: u64,
    },
    /// The commits a consumer asked for were pruned. It must resynchronise from a snapshot or
    /// rebuild; silently skipping them would lose changes.
    #[error(
        "commit sequence {requested} was pruned from the change log (earliest stored: {earliest})"
    )]
    Truncated {
        /// First sequence number the consumer needs.
        requested: u64,
        /// Oldest sequence number still stored.
        earliest: u64,
    },
}
