//! Persistent projections of the canonical log.
//!
//! * [`PersistentCurrentStore`] — the latest version of every row (OLTP path). Updates replace
//!   the tuple and free the old heap slot, so the store does not accumulate dead tuples.
//! * [`PersistentVersionStore`] — immutable historical versions indexed by `(row, begin_ts)`.
//! * [`CheckpointStore`] — the log position up to which both projections are persisted.
//!
//! None of these structures writes to disk on mutation. The engine publishes their dirty pages
//! atomically through the checkpoint journal ([`Checkpointable`]). Because the canonical log is
//! the source of truth, the projections can always be rebuilt from it.
pub mod checkpoint;
pub mod error;
pub mod heap;
pub mod integrity;
pub mod persistent_current;
pub mod persistent_version;
pub mod record;
pub mod stats;

pub use checkpoint::{Checkpoint, CheckpointStore, CHECKPOINT_FORMAT_VERSION};
pub use error::StorageError;
pub use heap::{HeapFile, SpaceReuse};
pub use integrity::{IntegrityChecker, IntegrityReport};
pub use persistent_current::PersistentCurrentStore;
pub use persistent_version::PersistentVersionStore;
pub use record::{CurrentRecord, HistoricalVersion};
pub use stats::StorageStats;

use adb_journal::FileWrite;

/// A structure whose in-memory changes are persisted by the checkpoint journal.
pub trait Checkpointable {
    /// Writes that move the on-disk files to the in-memory state.
    fn journal_writes(&self) -> Result<Vec<FileWrite>, StorageError>;
    /// Declares the state captured by `journal_writes` persisted.
    fn mark_clean(&self);
    /// Pages modified since the last checkpoint (drives the checkpoint budget).
    fn dirty_pages(&self) -> usize;
}
