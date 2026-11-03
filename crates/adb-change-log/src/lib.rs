//! The change log: the ordered stream of committed changes that projections consume.
//!
//! The canonical write-ahead log stays the source of truth. The change log is *derived* from it:
//! one [`ChangeEnvelope`] per commit (data or catalog), ordered by [`adb_core::CommitSeq`], in a
//! compact form that a consumer can read without understanding WAL framing. Because it is
//! derived, a lost or short change log is rebuilt from the WAL on restart.
//!
//! This crate is storage-engine agnostic: it knows rows and schema markers, not transactions
//! managers or indexes.
pub mod cursor;
pub mod error;
pub mod log;
pub mod model;
pub mod retention;
pub(crate) mod segment;

pub use cursor::{ChangeBatch, ChangeCursor};
pub use error::ChangeLogError;
pub use log::{AppendOutcome, ChangeLog, DEFAULT_SEGMENT_BYTES};
pub use model::{
    decode_envelope, encode_envelope, ChangeEnvelope, ChangePayload, RowChange, SchemaChangeMarker,
    MAX_ENVELOPE_BYTES,
};
pub use retention::{ChangeRetentionPolicy, PruneReport, RetentionFrontier};
