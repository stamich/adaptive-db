//! Page-level errors.
use thiserror::Error;

/// Page format and capacity errors.
#[derive(Debug, Error)]
pub enum PageError {
    /// Not enough free space for the tuple.
    #[error("page is full")]
    Full,
    /// The slot does not exist or is free.
    #[error("invalid slot {0}")]
    InvalidSlot(u16),
    /// The page violates a structural invariant.
    #[error("corrupt page: {0}")]
    Corrupt(String),
    /// The tuple can never fit in a page.
    #[error("payload too large: {0} bytes")]
    PayloadTooLarge(usize),
}
