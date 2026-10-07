//! Registry of snapshots held by live transactions.
//!
//! The engine needs the oldest active snapshot to know which delete tombstones can be vacuumed:
//! a tombstone committed at `t` is still needed while a transaction with a snapshot older than
//! `t` may try to write that row (its write must be detected as a conflict).

use std::{collections::BTreeMap, sync::Arc};

use adb_core::CommitTs;
use parking_lot::Mutex;

/// Multiset of active snapshot timestamps.
#[derive(Debug, Default)]
pub struct SnapshotRegistry {
    active: Mutex<BTreeMap<CommitTs, usize>>,
}

impl SnapshotRegistry {
    /// Registers a snapshot; it stays registered until the lease is dropped.
    pub fn register(self: &Arc<Self>, ts: CommitTs) -> SnapshotLease {
        *self.active.lock().entry(ts).or_insert(0) += 1;
        SnapshotLease {
            registry: Arc::clone(self),
            ts,
        }
    }

    /// Oldest registered snapshot.
    pub fn oldest(&self) -> Option<CommitTs> {
        self.active.lock().keys().next().copied()
    }

    /// Number of registered snapshots.
    pub fn len(&self) -> usize {
        self.active.lock().values().sum()
    }

    /// Whether no snapshot is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn release(&self, ts: CommitTs) {
        let mut active = self.active.lock();
        if let Some(count) = active.get_mut(&ts) {
            *count -= 1;
            if *count == 0 {
                active.remove(&ts);
            }
        }
    }
}

/// RAII registration of one snapshot. Dropping a transaction — committed, rolled back or simply
/// forgotten — always releases its snapshot.
#[derive(Debug)]
pub struct SnapshotLease {
    registry: Arc<SnapshotRegistry>,
    ts: CommitTs,
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        self.registry.release(self.ts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_track_the_oldest_snapshot() {
        let registry = Arc::new(SnapshotRegistry::default());
        let a = registry.register(CommitTs(5));
        let b = registry.register(CommitTs(3));
        let c = registry.register(CommitTs(3));
        assert_eq!(registry.oldest(), Some(CommitTs(3)));
        drop(b);
        assert_eq!(registry.oldest(), Some(CommitTs(3)));
        drop(c);
        assert_eq!(registry.oldest(), Some(CommitTs(5)));
        drop(a);
        assert!(registry.is_empty());
    }
}
