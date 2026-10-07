//! The canonical log: the source of truth of Adaptive DB.
//!
//! Every committed transaction is a contiguous run `Begin, Version*, (Put|Delete)*, Commit`
//! appended under the engine's commit lock. Projections (current/version stores) and the
//! change feed are both derived from this log.
pub mod cursor;
pub mod error;
pub mod format;
pub mod record;
pub mod segmented;

pub use cursor::{read_all, LogEntry, WalCursor};
pub use error::WalError;
pub use record::WalRecord;
pub use segmented::{
    earliest_lsn, lsn_offset, lsn_segment, make_lsn, SegmentedWalWriter, DEFAULT_SEGMENT_SIZE,
};
