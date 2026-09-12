//! Module `version_key` for crate `adb-core`.
use serde::{Deserialize, Serialize};

use crate::{CommitTs, RowId};

/// Lexicographically ordered temporal key.
/// Ordering is first by RowId, then by version begin timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Represents `VersionKey` state used by this subsystem.
pub struct VersionKey {
    pub row_id: RowId,
    pub begin_ts: CommitTs,
}

/// Implements behavior for `VersionKey`.
impl VersionKey {
    /// Defines the `fn` constant used by this subsystem.
    pub const fn new(row_id: RowId, begin_ts: CommitTs) -> Self {
        Self { row_id, begin_ts }
    }
}
