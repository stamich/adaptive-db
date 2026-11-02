//! Allocation of transaction ids and commit timestamps.
use std::sync::Arc;

use adb_core::{CommitResult, CommitSeq, CommitTs, TxId};

use crate::{CommitCoordinator, IsolationLevel, SnapshotRegistry, Transaction};

/// Hands out transaction ids and tracks live snapshots; commit identities come from its
/// [`CommitCoordinator`].
///
/// A commit identity is allocated under the commit lock but published (made visible to new
/// snapshots) only after its log records are durable.
#[derive(Debug)]
pub struct TransactionManager {
    /// Next transaction id to hand out.
    next_tx_id: u64,
    /// Allocation and publication of commit timestamps and sequence numbers.
    coordinator: CommitCoordinator,
    /// Snapshots of live transactions.
    snapshots: Arc<SnapshotRegistry>,
}

impl Default for TransactionManager {
    /// Ids and timestamps start at 1; nothing is committed yet.
    fn default() -> Self {
        Self {
            next_tx_id: 1,
            coordinator: CommitCoordinator::default(),
            snapshots: Arc::new(SnapshotRegistry::default()),
        }
    }
}

impl TransactionManager {
    /// Starts a transaction at the latest visible commit.
    pub fn begin(&mut self, isolation: IsolationLevel) -> Transaction {
        let id = TxId(self.next_tx_id);
        self.next_tx_id += 1;
        let snapshot = self.latest_committed_ts();
        Transaction::new(id, snapshot, isolation, self.snapshots.register(snapshot))
    }

    /// Reserves the next commit identity (not yet visible).
    pub fn allocate_commit(&mut self) -> CommitResult {
        self.coordinator.allocate()
    }

    /// Makes commits up to `commit` visible to new snapshots and to the canonical head.
    pub fn publish_commit(&mut self, commit: CommitResult) {
        self.coordinator.publish(commit);
    }

    /// Identity of the most recent commit handed out so far (visible or not).
    pub fn last_allocated_commit(&self) -> CommitResult {
        self.coordinator.last_allocated()
    }

    /// Latest visible commit.
    pub fn latest_committed_ts(&self) -> CommitTs {
        self.coordinator.published_ts()
    }

    /// Latest published commit sequence number: the canonical head.
    pub fn latest_commit_seq(&self) -> CommitSeq {
        self.coordinator.published_seq()
    }

    /// Oldest snapshot still held by a live transaction, or the latest commit if none.
    pub fn oldest_active_snapshot(&self) -> CommitTs {
        self.snapshots
            .oldest()
            .unwrap_or_else(|| self.latest_committed_ts())
    }

    /// Continues numbering after the state found by recovery.
    pub fn advance_after_recovery(
        &mut self,
        max_tx_id: u64,
        max_commit_ts: CommitTs,
        max_commit_seq: CommitSeq,
    ) {
        self.next_tx_id = self.next_tx_id.max(max_tx_id.saturating_add(1));
        self.coordinator
            .advance_after_recovery(max_commit_ts, max_commit_seq);
    }

    /// Highest transaction id handed out so far.
    pub fn last_tx_id(&self) -> u64 {
        self.next_tx_id - 1
    }
}

/// Unit tests of id allocation and validation.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{validate, Conflict};
    use adb_core::{Row, RowId};

    /// Lookup of the latest commit per row from a fixed `(row, ts)` table.
    fn latest(map: &[(u128, u64)]) -> impl FnMut(RowId) -> Result<Option<CommitTs>, ()> + '_ {
        move |row| {
            Ok(map
                .iter()
                .find(|(id, _)| RowId(*id) == row)
                .map(|(_, ts)| CommitTs(*ts)))
        }
    }

    /// Publishing commits out of order keeps the visible timestamp at the maximum.
    #[test]
    fn publication_is_monotonic_under_out_of_order_completion() {
        let mut manager = TransactionManager::default();
        let first = manager.allocate_commit();
        let second = manager.allocate_commit();
        manager.publish_commit(second);
        manager.publish_commit(first);
        assert_eq!(manager.latest_committed_ts(), second.commit_ts);
        assert_eq!(manager.latest_commit_seq(), second.commit_seq);
    }

    /// Dropping a transaction releases its snapshot from the horizon.
    #[test]
    fn dropped_transactions_release_their_snapshot() {
        let mut manager = TransactionManager::default();
        manager.advance_after_recovery(0, CommitTs(4), CommitSeq(4));
        let tx = manager.begin(IsolationLevel::Serializable);
        manager.advance_after_recovery(0, CommitTs(9), CommitSeq(9));
        assert_eq!(manager.oldest_active_snapshot(), CommitTs(4));
        drop(tx);
        assert_eq!(manager.oldest_active_snapshot(), CommitTs(9));
    }

    /// A concurrent write to a written row aborts under both isolation levels.
    #[test]
    fn write_write_conflicts_abort_at_every_level() {
        for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
            let mut tx = TransactionManager::default().begin(isolation);
            tx.put(RowId(1), Row::new());
            assert_eq!(
                validate(&tx, latest(&[(1, 1)])).unwrap(),
                Err(Conflict::WriteWrite(RowId(1)))
            );
        }
    }

    /// A concurrent write to a read row aborts only under serializable isolation.
    #[test]
    fn read_write_conflicts_abort_only_when_serializable() {
        let mut snapshot = TransactionManager::default().begin(IsolationLevel::Snapshot);
        snapshot.record_read(RowId(1));
        snapshot.put(RowId(2), Row::new());
        assert_eq!(validate(&snapshot, latest(&[(1, 1)])).unwrap(), Ok(()));

        let mut serializable = TransactionManager::default().begin(IsolationLevel::Serializable);
        serializable.record_read(RowId(1));
        serializable.put(RowId(2), Row::new());
        assert_eq!(
            validate(&serializable, latest(&[(1, 1)])).unwrap(),
            Err(Conflict::ReadWrite(RowId(1)))
        );
    }
}
