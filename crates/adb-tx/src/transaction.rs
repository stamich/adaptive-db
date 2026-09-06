//! Transaction module for the adb-tx crate.
//!
use std::collections::HashMap;

use adb_core::{CommitTs, Row, RowId, TxId};

use crate::Mutation;

/// Represents one MVCC transaction, including its snapshot timestamp and transaction-local write set.
#[derive(Debug)]
pub struct Transaction {
    pub(crate) id: TxId,
    pub(crate) snapshot_ts: CommitTs,
    pub(crate) writes: HashMap<RowId, Mutation>,
    pub(crate) closed: bool,
}

/// Implements behavior for `Transaction`.
impl Transaction {
    /// Creates a new instance initialized with the supplied state.
    pub(crate) fn new(id: TxId, snapshot_ts: CommitTs) -> Self {
        Self {
            id,
            snapshot_ts,
            writes: HashMap::new(),
            closed: false,
        }
    }

    /// Implements the `id` operation used by this subsystem.
    pub fn id(&self) -> TxId {
        self.id
    }

    /// Implements the `snapshot_ts` operation used by this subsystem.
    pub fn snapshot_ts(&self) -> CommitTs {
        self.snapshot_ts
    }

    /// Implements the `put` operation used by this subsystem.
    pub fn put(&mut self, row_id: RowId, row: Row) {
        self.writes.insert(row_id, Mutation::Put(row));
    }

    /// Implements the `delete` operation used by this subsystem.
    pub fn delete(&mut self, row_id: RowId) {
        self.writes.insert(row_id, Mutation::Delete);
    }

    /// Implements the `local_read` operation used by this subsystem.
    pub fn local_read(&self, row_id: RowId) -> Option<Option<Row>> {
        match self.writes.get(&row_id) {
            Some(Mutation::Put(row)) => Some(Some(row.clone())),
            Some(Mutation::Delete) => Some(None),
            None => None,
        }
    }

    /// Implements the `writes` operation used by this subsystem.
    pub fn writes(&self) -> &HashMap<RowId, Mutation> {
        &self.writes
    }

    /// Implements the `mark_closed` operation used by this subsystem.
    pub fn mark_closed(&mut self) {
        self.closed = true;
    }

    /// Returns whether the `closed` condition holds.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
