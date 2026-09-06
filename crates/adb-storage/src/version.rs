//! Version module for the adb-storage crate.
//!
use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId};

/// Represents `HistoricalVersion` state used by the src subsystem.
#[derive(Debug, Clone)]
pub struct HistoricalVersion {
    pub begin_ts: CommitTs,
    pub end_ts: CommitTs,
    pub value: Option<Row>,
}

/// Represents `VersionStore` state used by the src subsystem.
#[derive(Debug, Default)]
pub struct VersionStore {
    versions: HashMap<RowId, Vec<HistoricalVersion>>,
}

/// Implements behavior for `VersionStore`.
impl VersionStore {
    /// Appends data to the `push` collection maintained by this subsystem.
    pub fn push(&mut self, row_id: RowId, version: HistoricalVersion) {
        self.versions.entry(row_id).or_default().push(version);
    }

    /// Returns the row version visible at the supplied historical commit timestamp.
    pub fn get_at(
        &self,
        row_id: RowId,
        ts: CommitTs,
    ) -> Option<&HistoricalVersion> {
        self.versions
            .get(&row_id)?
            .iter()
            .rev()
            .find(|v| v.begin_ts <= ts && ts < v.end_ts)
    }

    /// Implements the `all_for` operation used by this subsystem.
    pub fn all_for(&self, row_id: RowId) -> &[HistoricalVersion] {
        self.versions
            .get(&row_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
