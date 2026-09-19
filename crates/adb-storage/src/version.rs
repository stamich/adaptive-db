//! Module `version` for crate `adb-storage`.
use adb_core::{CommitTs, Row, RowId};
use serde::{Deserialize, Serialize};

/// Represents `HistoricalVersion` state used by this subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoricalVersion {
    pub begin_ts: CommitTs,
    pub end_ts: CommitTs,
    pub value: Option<Row>,
}

/// Implements behavior for `HistoricalVersion`.
impl HistoricalVersion {
    /// Implements the `visible_at` operation used by this subsystem.
    pub fn visible_at(&self, ts: CommitTs) -> bool {
        self.begin_ts <= ts && ts < self.end_ts
    }
}

/// Compatibility in-memory implementation retained for focused unit tests.
#[derive(Debug, Default)]
pub struct VersionStore {
    versions: std::collections::HashMap<RowId, Vec<HistoricalVersion>>,
}

/// Implements behavior for `VersionStore`.
impl VersionStore {
    /// Implements the `push` operation used by this subsystem.
    pub fn push(&mut self, row_id: RowId, version: HistoricalVersion) {
        self.versions.entry(row_id).or_default().push(version);
    }

    /// Implements the `get_at` operation used by this subsystem.
    pub fn get_at(&self, row_id: RowId, ts: CommitTs) -> Option<&HistoricalVersion> {
        self.versions
            .get(&row_id)?
            .iter()
            .rev()
            .find(|version| version.visible_at(ts))
    }

    /// Implements the `all_for` operation used by this subsystem.
    pub fn all_for(&self, row_id: RowId) -> &[HistoricalVersion] {
        self.versions.get(&row_id).map(Vec::as_slice).unwrap_or(&[])
    }
}
