//! Recovery module for the adb-engine crate.
//!
use std::collections::HashMap;

use adb_core::{CommitTs, RowId, TxId};
use adb_storage::Stores;
use adb_tx::Mutation;
use adb_wal::WalRecord;

/// Represents `PendingTx` state used by the src subsystem.
#[derive(Debug, Default)]
struct PendingTx {
    mutations: Vec<(RowId, Mutation)>,
}

/// Represents `RecoveryResult` state used by the src subsystem.
#[derive(Debug, Default)]
pub struct RecoveryResult {
    pub stores: Stores,
    pub max_tx_id: u64,
    pub max_commit_ts: u64,
}

/// Replays committed WAL transactions into reconstructed in-memory stores.
pub fn recover(records: impl IntoIterator<Item = WalRecord>) -> RecoveryResult {
    let mut pending: HashMap<TxId, PendingTx> = HashMap::new();
    let mut result = RecoveryResult::default();

    for record in records {
        match record {
            WalRecord::Begin { tx_id, .. } => {
                result.max_tx_id = result.max_tx_id.max(tx_id.0);
                pending.entry(tx_id).or_default();
            }

            WalRecord::Put {
                tx_id,
                row_id,
                value,
            } => {
                result.max_tx_id = result.max_tx_id.max(tx_id.0);
                pending
                    .entry(tx_id)
                    .or_default()
                    .mutations
                    .push((row_id, Mutation::Put(value)));
            }

            WalRecord::Delete { tx_id, row_id } => {
                result.max_tx_id = result.max_tx_id.max(tx_id.0);
                pending
                    .entry(tx_id)
                    .or_default()
                    .mutations
                    .push((row_id, Mutation::Delete));
            }

            WalRecord::Commit { tx_id, commit_ts } => {
                result.max_tx_id = result.max_tx_id.max(tx_id.0);
                result.max_commit_ts = result.max_commit_ts.max(commit_ts.0);

                if let Some(pending_tx) = pending.remove(&tx_id) {
                    apply_mutations(&mut result.stores, pending_tx.mutations, commit_ts);
                }
            }

            WalRecord::Abort { tx_id } => {
                result.max_tx_id = result.max_tx_id.max(tx_id.0);
                pending.remove(&tx_id);
            }
        }
    }

    result
}

/// Applies a committed transaction's mutations to the reconstructed stores.
fn apply_mutations(stores: &mut Stores, mutations: Vec<(RowId, Mutation)>, commit_ts: CommitTs) {
    for (row_id, mutation) in mutations {
        match mutation {
            Mutation::Put(row) => stores.apply_put(row_id, row, commit_ts),
            Mutation::Delete => stores.apply_delete(row_id, commit_ts),
        }
    }
}
