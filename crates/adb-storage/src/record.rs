//! Values stored in the current and version projections.
use adb_core::{CommitTs, Row};
use serde::{Deserialize, Serialize};

/// Latest committed state of a row. `value == None` is a delete tombstone, kept until vacuum
/// proves no active transaction can still conflict with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentRecord {
    /// Commit that produced this state.
    pub commit_ts: CommitTs,
    /// Row content, or `None` for a tombstone.
    pub value: Option<Row>,
}

/// A superseded row state, visible in the half-open interval `[begin_ts, end_ts)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoricalVersion {
    /// Commit that produced the state.
    pub begin_ts: CommitTs,
    /// Commit that replaced or deleted it.
    pub end_ts: CommitTs,
    /// Row content, or `None` when the state was itself a tombstone.
    pub value: Option<Row>,
}

impl HistoricalVersion {
    /// Whether a snapshot at `ts` sees this version.
    pub fn visible_at(&self, ts: CommitTs) -> bool {
        self.begin_ts <= ts && ts < self.end_ts
    }
}
