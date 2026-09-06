use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId};

#[derive(Debug, Clone)]
pub struct HistoricalVersion {
    pub begin_ts: CommitTs,
    pub end_ts: CommitTs,
    pub value: Option<Row>,
}

#[derive(Debug, Default)]
pub struct VersionStore {
    versions: HashMap<RowId, Vec<HistoricalVersion>>,
}

impl VersionStore {
    pub fn push(&mut self, row_id: RowId, version: HistoricalVersion) {
        self.versions.entry(row_id).or_default().push(version);
    }

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

    pub fn all_for(&self, row_id: RowId) -> &[HistoricalVersion] {
        self.versions
            .get(&row_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
