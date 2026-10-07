//! Allocation of transaction ids and commit timestamps.
use std::sync::Arc;

use adb_core::{CommitTs, TxId};

use crate::{IsolationLevel, SnapshotRegistry, Transaction};

/// Hands out ids/timestamps and tracks the latest *visible* commit.
///
/// A commit timestamp is allocated under the commit lock but published (made visible to new
/// snapshots) only after its log records are durable. Group commit can finish commits out of
/// allocation order; publication is therefore a monotonic maximum, which is safe because a
/// durable later commit implies every earlier appended commit is durable too.
#[derive(Debug)]
pub struct TransactionManager {
    next_tx_id: u64,
    next_commit_ts: u64,
    last_committed_ts: u64,
    snapshots: Arc<SnapshotRegistry>,
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self {
            next_tx_id: 1,
            next_commit_ts: 1,
            last_committed_ts: 0,
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

    /// Reserves the next commit timestamp (not yet visible).
    pub fn allocate_commit_ts(&mut self) -> CommitTs {
        let ts = CommitTs(self.next_commit_ts);
        self.next_commit_ts += 1;
        ts
    }

    /// Makes commits up to `commit_ts` visible to new snapshots.
    pub fn publish_commit(&mut self, commit_ts: CommitTs) {
        self.last_committed_ts = self.last_committed_ts.max(commit_ts.0);
    }

    /// Highest commit timestamp handed out so far (visible or not).
    pub fn last_allocated_commit_ts(&self) -> CommitTs {
        CommitTs(self.next_commit_ts - 1)
    }

    /// Latest visible commit.
    pub fn latest_committed_ts(&self) -> CommitTs {
        CommitTs(self.last_committed_ts)
    }

    /// Oldest snapshot still held by a live transaction, or the latest commit if none.
    pub fn oldest_active_snapshot(&self) -> CommitTs {
        self.snapshots
            .oldest()
            .unwrap_or_else(|| self.latest_committed_ts())
    }

    /// Continues numbering after the state found by recovery.
    pub fn advance_after_recovery(&mut self, max_tx_id: u64, max_commit_ts: u64) {
        self.next_tx_id = self.next_tx_id.max(max_tx_id.saturating_add(1));
        self.next_commit_ts = self.next_commit_ts.max(max_commit_ts.saturating_add(1));
        self.last_committed_ts = self.last_committed_ts.max(max_commit_ts);
    }

    /// Highest transaction id handed out so far.
    pub fn last_tx_id(&self) -> u64 {
        self.next_tx_id - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{validate, Conflict};
    use adb_core::{Row, RowId};

    fn latest(map: &[(u128, u64)]) -> impl FnMut(RowId) -> Result<Option<CommitTs>, ()> + '_ {
        move |row| {
            Ok(map
                .iter()
                .find(|(id, _)| RowId(*id) == row)
                .map(|(_, ts)| CommitTs(*ts)))
        }
    }

    #[test]
    fn publication_is_monotonic_under_out_of_order_completion() {
        let mut manager = TransactionManager::default();
        let first = manager.allocate_commit_ts();
        let second = manager.allocate_commit_ts();
        manager.publish_commit(second);
        manager.publish_commit(first);
        assert_eq!(manager.latest_committed_ts(), second);
    }

    #[test]
    fn dropped_transactions_release_their_snapshot() {
        let mut manager = TransactionManager::default();
        manager.publish_commit(CommitTs(4));
        let tx = manager.begin(IsolationLevel::Serializable);
        manager.publish_commit(CommitTs(9));
        assert_eq!(manager.oldest_active_snapshot(), CommitTs(4));
        drop(tx);
        assert_eq!(manager.oldest_active_snapshot(), CommitTs(9));
    }

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
