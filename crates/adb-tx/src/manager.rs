use adb_core::{CommitTs, TxId};

use crate::Transaction;

#[derive(Debug)]
pub struct TransactionManager {
    next_tx_id: u64,
    next_commit_ts: u64,
    last_committed_ts: u64,
}

impl TransactionManager {
    pub fn new(
        next_tx_id: u64,
        next_commit_ts: u64,
        last_committed_ts: u64,
    ) -> Self {
        Self {
            next_tx_id,
            next_commit_ts,
            last_committed_ts,
        }
    }

    pub fn begin(&mut self) -> Transaction {
        let id = TxId(self.next_tx_id);
        self.next_tx_id += 1;

        Transaction::new(id, self.latest_committed_ts())
    }

    /// Reserves a unique commit timestamp, but does NOT make it visible
    /// to newly-started transactions yet.
    pub fn allocate_commit_ts(&mut self) -> CommitTs {
        let ts = CommitTs(self.next_commit_ts);
        self.next_commit_ts += 1;
        ts
    }

    /// Publishes the timestamp after the durable WAL record has been synced
    /// and the committed mutation has been installed in the in-memory stores.
    pub fn publish_commit(&mut self, commit_ts: CommitTs) {
        assert!(
            commit_ts.0 > self.last_committed_ts,
            "commit timestamps must be published monotonically"
        );
        self.last_committed_ts = commit_ts.0;
    }

    pub fn latest_committed_ts(&self) -> CommitTs {
        CommitTs(self.last_committed_ts)
    }

    pub fn advance_after_recovery(&mut self, max_tx_id: u64, max_commit_ts: u64) {
        self.next_tx_id = self.next_tx_id.max(max_tx_id.saturating_add(1));
        self.next_commit_ts = self
            .next_commit_ts
            .max(max_commit_ts.saturating_add(1));
        self.last_committed_ts = self.last_committed_ts.max(max_commit_ts);
    }
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new(1, 1, 0)
    }
}
