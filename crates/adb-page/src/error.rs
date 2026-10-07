//! Page-level errors.
use thiserror::Error;

/// Page format and capacity errors.
#[derive(Debug, Error)]
pub enum PageError {
    #[error("page is full")]
    Full,
    #[error("invalid slot {0}")]
    InvalidSlot(u16),
    #[error("corrupt page: {0}")]
    Corrupt(String),
    #[error("payload too large: {0} bytes")]
    PayloadTooLarge(usize),
}
