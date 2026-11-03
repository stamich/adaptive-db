//! The commit coordinator: one place that hands out commit identities and publishes them.
//!
//! Every commit, data or catalog, receives a [`CommitTs`] (the MVCC timestamp its rows carry)
//! and a [`CommitSeq`] (its place in the global commit order) from the same call, under the
//! engine's commit lock. Publication happens later, once the commit's log records are durable.
//! The coordinator keeps the two counters apart on purpose: today both advance by one per
//! commit, but the timestamp is an MVCC concept and the sequence number is the coordinate of
//! the change log, of session tokens and of projection frontiers.

use adb_core::{CommitResult, CommitSeq, CommitTs};

/// Allocates and publishes commit timestamps and sequence numbers.
///
/// Group commit can finish commits out of allocation order. Publication is therefore a
/// monotonic maximum, which is safe because a durable later commit implies that every earlier
/// appended commit is durable too.
#[derive(Debug, Clone)]
pub struct CommitCoordinator {
    /// Next commit timestamp to hand out.
    next_ts: u64,
    /// Next commit sequence number to hand out.
    next_seq: u64,
    /// Latest commit timestamp visible to new snapshots.
    published_ts: u64,
    /// Latest commit sequence number visible to readers (the canonical head).
    published_seq: u64,
}

impl Default for CommitCoordinator {
    /// Numbering starts at 1; nothing is committed yet.
    fn default() -> Self {
        Self {
            next_ts: 1,
            next_seq: 1,
            published_ts: 0,
            published_seq: 0,
        }
    }
}

impl CommitCoordinator {
    /// Reserves the next commit identity (not yet visible).
    pub fn allocate(&mut self) -> CommitResult {
        let result = CommitResult {
            commit_ts: CommitTs(self.next_ts),
            commit_seq: CommitSeq(self.next_seq),
        };
        self.next_ts += 1;
        self.next_seq += 1;
        result
    }

    /// Makes commits up to `commit` visible to new snapshots and to the canonical head.
    pub fn publish(&mut self, commit: CommitResult) {
        self.published_ts = self.published_ts.max(commit.commit_ts.0);
        self.published_seq = self.published_seq.max(commit.commit_seq.0);
    }

    /// The most recent identity handed out, visible or not (zeroes before the first commit).
    pub fn last_allocated(&self) -> CommitResult {
        CommitResult {
            commit_ts: CommitTs(self.next_ts - 1),
            commit_seq: CommitSeq(self.next_seq - 1),
        }
    }

    /// Latest commit timestamp visible to new snapshots.
    pub fn published_ts(&self) -> CommitTs {
        CommitTs(self.published_ts)
    }

    /// Latest published sequence number: the canonical head readers can rely on.
    pub fn published_seq(&self) -> CommitSeq {
        CommitSeq(self.published_seq)
    }

    /// Continues numbering after the highest identities found by recovery; everything found is
    /// durable, so it counts as published.
    pub fn advance_after_recovery(&mut self, max_ts: CommitTs, max_seq: CommitSeq) {
        self.next_ts = self.next_ts.max(max_ts.0.saturating_add(1));
        self.next_seq = self.next_seq.max(max_seq.0.saturating_add(1));
        self.published_ts = self.published_ts.max(max_ts.0);
        self.published_seq = self.published_seq.max(max_seq.0);
    }
}

/// Unit tests of allocation and publication.
#[cfg(test)]
mod tests {
    use super::*;

    /// Identities are consecutive and allocation alone publishes nothing.
    #[test]
    fn allocation_is_consecutive_and_invisible_until_published() {
        let mut coordinator = CommitCoordinator::default();
        let first = coordinator.allocate();
        let second = coordinator.allocate();
        assert_eq!(first.commit_seq, CommitSeq(1));
        assert_eq!(second.commit_seq, CommitSeq(2));
        assert_eq!(second.commit_ts, CommitTs(2));
        assert_eq!(coordinator.published_seq(), CommitSeq::ZERO);
        assert_eq!(coordinator.last_allocated(), second);
    }

    /// Publishing out of order keeps the head at the maximum.
    #[test]
    fn publication_is_monotonic() {
        let mut coordinator = CommitCoordinator::default();
        let first = coordinator.allocate();
        let second = coordinator.allocate();
        coordinator.publish(second);
        coordinator.publish(first);
        assert_eq!(coordinator.published_seq(), CommitSeq(2));
        assert_eq!(coordinator.published_ts(), CommitTs(2));
    }

    /// After recovery numbering continues above what the log holds, and that is published.
    #[test]
    fn recovery_continues_numbering_and_publishes_the_log() {
        let mut coordinator = CommitCoordinator::default();
        coordinator.advance_after_recovery(CommitTs(10), CommitSeq(12));
        assert_eq!(coordinator.published_seq(), CommitSeq(12));
        let next = coordinator.allocate();
        assert_eq!(next.commit_ts, CommitTs(11));
        assert_eq!(next.commit_seq, CommitSeq(13));
    }
}
