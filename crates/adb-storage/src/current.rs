//! Current module for the adb-storage crate.
//!
use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId};

/// Represents `CurrentRecord` state used by the src subsystem.
#[derive(Debug, Clone)]
pub struct CurrentRecord {
    pub commit_ts: CommitTs,
    pub value: Option<Row>,
}

/// Represents `CurrentStore` state used by the src subsystem.
#[derive(Debug, Default)]
pub struct CurrentStore {
    rows: HashMap<RowId, CurrentRecord>,
}

/// Implements behavior for `CurrentStore`.
impl CurrentStore {
    /// Returns the value visible for the requested key or row at the operation's default snapshot.
    pub fn get(&self, row_id: RowId) -> Option<&CurrentRecord> {
        self.rows.get(&row_id)
    }

    /// Inserts a new item into the underlying page, heap, tree, or transaction-local mutation set.
    pub fn insert(
        &mut self,
        row_id: RowId,
        record: CurrentRecord,
    ) -> Option<CurrentRecord> {
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
