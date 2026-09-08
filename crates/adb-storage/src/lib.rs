//! Lib module for the adb-storage crate.
//!
pub mod checkpoint;
pub mod current;
pub mod error;
pub mod heap;
pub mod persistent_current;
pub mod stores;
pub mod version;

pub use checkpoint::{Checkpoint, CheckpointStore};
pub use current::{CurrentRecord, CurrentStore};
pub use error::StorageError;
pub use heap::HeapFile;
pub use persistent_current::PersistentCurrentStore;
pub use stores::Stores;
pub use version::{HistoricalVersion, VersionStore};