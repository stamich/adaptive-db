//! Value types shared by every Adaptive DB crate: identifiers, rows, values and key ranges.
//!
//! This crate has no I/O and no engine logic; it only defines the vocabulary the other
//! layers use to talk to each other.
pub mod error;
pub mod ids;
pub mod key_range;
pub mod location;
pub mod row;
pub mod value;
pub mod version_key;

pub use error::CoreError;
pub use ids::{CommitTs, FieldId, Lsn, PageId, RowId, SlotId, TxId};
pub use key_range::KeyRange;
pub use location::RowLocation;
pub use row::Row;
pub use value::Value;
pub use version_key::VersionKey;
