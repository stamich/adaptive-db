//! Composite key of the temporal (version) index.
use serde::{Deserialize, Serialize};

use crate::{CommitTs, RowId};

/// Orders historical versions by row first and by version start second, so all versions of a
/// row are adjacent and a floor lookup at `(row, ts)` finds the version visible at `ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VersionKey {
    /// Row the version belongs to.
    pub row_id: RowId,
    /// Commit timestamp at which the version became visible.
    pub begin_ts: CommitTs,
}

impl VersionKey {
    /// Creates a key for the version of `row_id` that started at `begin_ts`.
    pub const fn new(row_id: RowId, begin_ts: CommitTs) -> Self {
        Self { row_id, begin_ts }
    }
}
