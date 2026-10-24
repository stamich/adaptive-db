//! Secondary indexes of Adaptive DB (Milestone 2.3.0).
//!
//! An index is one more **derived projection of the canonical log**, like the current-state
//! and version stores: it is maintained from the same committed mutations and before-images,
//! published by the same checkpoint journal and rebuilt from the log when it is lost. It never
//! writes log records of its own.
//!
//! * [`definition`] — what an index covers ([`IndexSpec`], [`IndexDefinition`]), its lifecycle
//!   ([`IndexState`]), validation and limits;
//! * [`entry`] — MVCC-versioned entries `(key, row id, begin) -> (end, covered values)` over an
//!   order-preserving key encoding, so snapshot reads at older timestamps stay correct.
pub mod definition;
pub mod entry;
pub mod error;

pub use definition::{
    IndexColumn, IndexDefinition, IndexSpec, IndexState, MAX_INCLUDE_BYTES, MAX_INCLUDE_COLUMNS,
    MAX_INDEX_KEY_BYTES, MAX_INDEX_NAME_BYTES, MAX_KEY_COLUMNS,
};
pub use entry::{IndexEntry, OPEN};
pub use error::IndexError;
