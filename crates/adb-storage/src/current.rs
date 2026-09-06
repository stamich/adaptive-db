use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId};

#[derive(Debug, Clone)]
pub struct CurrentRecord {
    pub commit_ts: CommitTs,
    pub value: Option<Row>,
}

#[derive(Debug, Default)]
pub struct CurrentStore {
    rows: HashMap<RowId, CurrentRecord>,
}

impl CurrentStore {
    pub fn get(&self, row_id: RowId) -> Option<&CurrentRecord> {
        self.rows.get(&row_id)
    }

    pub fn insert(
        &mut self,
        row_id: RowId,
        record: CurrentRecord,
    ) -> Option<CurrentRecord> {
        self.rows.insert(row_id, record)
    }

    pub fn remove(&mut self, row_id: RowId) -> Option<CurrentRecord> {
        self.rows.remove(&row_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RowId, &CurrentRecord)> {
        self.rows.iter()
    }
}
