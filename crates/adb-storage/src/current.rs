//! Module `current` for crate `adb-storage`.
use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId};
use serde::{Deserialize, Serialize};

/// Represents `CurrentRecord` state used by this subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentRecord {
    pub commit_ts: CommitTs,
    pub value: Option<Row>,
}

/// Represents `CurrentStore` state used by this subsystem.
#[derive(Debug, Default)]
pub struct CurrentStore {
    rows: HashMap<RowId, CurrentRecord>,
}

/// Implements behavior for `CurrentStore`.
impl CurrentStore {
    /// Implements the `get` operation used by this subsystem.
    pub fn get(&self, row_id: RowId) -> Option<&CurrentRecord> {
        self.rows.get(&row_id)
    }
    /// Implements the `insert` operation used by this subsystem.
    pub fn insert(&mut self, row_id: RowId, record: CurrentRecord) -> Option<CurrentRecord> {
        self.rows.insert(row_id, record)
    }
    /// Implements the `remove` operation used by this subsystem.
    pub fn remove(&mut self, row_id: RowId) -> Option<CurrentRecord> {
        self.rows.remove(&row_id)
    }
    /// Implements the `iter` operation used by this subsystem.
    pub fn iter(&self) -> impl Iterator<Item = (&RowId, &CurrentRecord)> {
        self.rows.iter()
    }
}
