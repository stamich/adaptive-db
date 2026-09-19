//! Module `lib` for crate `adb-storage`.
pub mod checkpoint;
pub mod current;
pub mod error;
pub mod heap;
pub mod integrity;
pub mod persistent_current;
pub mod persistent_version;
pub mod stats;
pub mod stores;
pub mod version;

pub use checkpoint::{Checkpoint, CheckpointStore, CHECKPOINT_FORMAT_VERSION};
pub use current::{CurrentRecord, CurrentStore};
pub use error::StorageError;
pub use heap::HeapFile;
pub use integrity::{IntegrityChecker, IntegrityReport};
pub use persistent_current::PersistentCurrentStore;
pub use persistent_version::PersistentVersionStore;
pub use stats::StorageStats;
pub use stores::Stores;
pub use version::{HistoricalVersion, VersionStore};
