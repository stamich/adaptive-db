//! Module `lib` for crate `adb-wal`.
pub mod error;
pub mod format;
pub mod reader;
pub mod record;
pub mod segmented;
pub mod writer;

pub use error::WalError;
pub use reader::WalReader;
pub use record::WalRecord;
pub use segmented::{
    lsn_offset, lsn_segment, make_lsn, SegmentedWalReader, SegmentedWalWriter, DEFAULT_SEGMENT_SIZE,
};
pub use writer::WalWriter;
