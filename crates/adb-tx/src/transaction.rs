use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId, TxId};

use crate::Mutation;

#[derive(Debug)]
pub struct Transaction {
    pub(crate) id: TxId,
    pub(crate) snapshot_ts: CommitTs,
    pub(crate) writes: HashMap<RowId, Mutation>,
    pub(crate) closed: bool,
}

impl Transaction {
    pub(crate) fn new(id: TxId, snapshot_ts: CommitTs) -> Self {
        Self {
            id,
            snapshot_ts,
            writes: HashMap::new(),
            closed: false,
        }
    }

    pub fn id(&self) -> TxId {
        self.id
    }

    pub fn snapshot_ts(&self) -> CommitTs {
        self.snapshot_ts
    }

    pub fn put(&mut self, row_id: RowId, row: Row) {
        self.writes.insert(row_id, Mutation::Put(row));
    }

    pub fn delete(&mut self, row_id: RowId) {
        self.writes.insert(row_id, Mutation::Delete);
    }

    pub fn local_read(&self, row_id: RowId) -> Option<Option<Row>> {
        match self.writes.get(&row_id) {
            Some(Mutation::Put(row)) => Some(Some(row.clone())),
            Some(Mutation::Delete) => Some(None),
            None => None,
        }
    }

    pub fn writes(&self) -> &HashMap<RowId, Mutation> {
        &self.writes
    }

    pub fn mark_closed(&mut self) {
        self.closed = true;
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
