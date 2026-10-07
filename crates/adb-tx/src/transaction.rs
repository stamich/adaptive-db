//! One optimistic transaction.
use std::collections::{BTreeMap, BTreeSet};

use adb_core::{CommitTs, Row, RowId, TxId};

use crate::{IsolationLevel, Mutation, SnapshotLease};

/// Buffered writes and recorded reads of one transaction attempt.
///
/// Writes are kept ordered by row id so the log and the change feed are deterministic.
#[derive(Debug)]
pub struct Transaction {
    /// Transaction id.
    id: TxId,
    /// Snapshot all reads observe.
    snapshot_ts: CommitTs,
    /// Validation rule applied at commit.
    isolation: IsolationLevel,
    /// Buffered writes by row.
    writes: BTreeMap<RowId, Mutation>,
    /// Rows read from the snapshot.
    reads: BTreeSet<RowId>,
    /// Set once committed or rolled back.
    closed: bool,
    /// Keeps the snapshot registered while the transaction lives.
    _lease: SnapshotLease,
}

impl Transaction {
    /// Creates a transaction; only the manager does this, so every one holds a lease.
    pub(crate) fn new(
        id: TxId,
        snapshot_ts: CommitTs,
        isolation: IsolationLevel,
        lease: SnapshotLease,
    ) -> Self {
        Self {
            id,
            snapshot_ts,
            isolation,
            writes: BTreeMap::new(),
            reads: BTreeSet::new(),
            closed: false,
            _lease: lease,
        }
    }

    /// Transaction id.
    pub fn id(&self) -> TxId {
        self.id
    }

    /// Snapshot all reads observe.
    pub fn snapshot_ts(&self) -> CommitTs {
        self.snapshot_ts
    }

    /// Isolation level validated at commit.
    pub fn isolation(&self) -> IsolationLevel {
        self.isolation
    }

    /// Buffers an insert/replace.
    pub fn put(&mut self, row_id: RowId, row: Row) {
        self.writes.insert(row_id, Mutation::Put(row));
    }

    /// Buffers a delete.
    pub fn delete(&mut self, row_id: RowId) {
        self.writes.insert(row_id, Mutation::Delete);
    }

    /// The transaction's own pending value of `row_id`: `Some(None)` = deleted here.
    pub fn local_read(&self, row_id: RowId) -> Option<Option<Row>> {
        self.writes.get(&row_id).map(|mutation| match mutation {
            Mutation::Put(row) => Some(row.clone()),
            Mutation::Delete => None,
        })
    }

    /// Records that `row_id` was read from the snapshot (validated under `Serializable`).
    pub fn record_read(&mut self, row_id: RowId) {
        self.reads.insert(row_id);
    }

    /// Buffered writes in row order.
    pub fn writes(&self) -> &BTreeMap<RowId, Mutation> {
        &self.writes
    }

    /// Rows read from the snapshot.
    pub fn reads(&self) -> &BTreeSet<RowId> {
        &self.reads
    }

    /// Whether the transaction has nothing to write.
    pub fn is_read_only(&self) -> bool {
        self.writes.is_empty()
    }

    /// Marks the transaction finished; later use is rejected by the engine.
    pub fn mark_closed(&mut self) {
        self.closed = true;
    }

    /// Whether the transaction already finished.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
