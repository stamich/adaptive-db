//! Logical WAL recovery analysis with transaction-state validation.

use std::collections::{HashMap, HashSet};

use adb_core::{CommitTs, Lsn, RowId, TxId};
use adb_storage::HistoricalVersion;
use adb_tx::Mutation;
use adb_wal::{WalError, WalRecord};

/// Accumulates not-yet-committed history and current-state mutations for one begun transaction.
#[derive(Debug, Default)]
pub struct PendingTx {
    /// Historical before-images emitted by the transaction.
    pub versions: Vec<(RowId, HistoricalVersion)>,
    /// Current-state mutations emitted by the transaction.
    pub mutations: Vec<(RowId, Mutation)>,
}

/// Captures the highest durable transaction/timestamp ids observed during recovery analysis.
#[derive(Debug, Default)]
pub struct RecoverySummary {
    /// Highest transaction id observed.
    pub max_tx_id: u64,
    /// Highest committed timestamp observed.
    pub max_commit_ts: u64,
}

/// Computes monotonic id/timestamp maxima from already frame-validated WAL records.
pub fn analyze(records: impl IntoIterator<Item = WalRecord>) -> RecoverySummary {
    let mut summary = RecoverySummary::default();
    for record in records {
        match record {
            WalRecord::Begin { tx_id, .. }
            | WalRecord::Version { tx_id, .. }
            | WalRecord::Put { tx_id, .. }
            | WalRecord::Delete { tx_id, .. }
            | WalRecord::Abort { tx_id } => {
                summary.max_tx_id = summary.max_tx_id.max(tx_id.0);
            }
            WalRecord::Commit { tx_id, commit_ts } => {
                summary.max_tx_id = summary.max_tx_id.max(tx_id.0);
                summary.max_commit_ts = summary.max_commit_ts.max(commit_ts.0);
            }
        }
    }
    summary
}

/// Reconstructs committed transactions while rejecting impossible logical WAL state transitions.
pub fn collect_committed(
    entries: &[(Lsn, WalRecord)],
) -> Result<Vec<(Lsn, TxId, CommitTs, PendingTx)>, WalError> {
    let mut pending: HashMap<TxId, PendingTx> = HashMap::new();
    let mut finished_ids = HashSet::new();
    let mut committed = Vec::new();

    for (lsn, record) in entries {
        match record {
            WalRecord::Begin { tx_id, .. } => {
                if finished_ids.contains(tx_id)
                    || pending.insert(*tx_id, PendingTx::default()).is_some()
                {
                    return Err(WalError::Corrupt(format!(
                        "duplicate/reused BEGIN for tx {}",
                        tx_id.0
                    )));
                }
            }
            WalRecord::Version {
                tx_id,
                row_id,
                begin_ts,
                end_ts,
                value,
            } => {
                if begin_ts >= end_ts {
                    return Err(WalError::Corrupt(format!(
                        "invalid historical interval for tx {}",
                        tx_id.0
                    )));
                }
                let tx = pending.get_mut(tx_id).ok_or_else(|| {
                    WalError::Corrupt(format!("VERSION without BEGIN for tx {}", tx_id.0))
                })?;
                tx.versions.push((
                    *row_id,
                    HistoricalVersion {
                        begin_ts: *begin_ts,
                        end_ts: *end_ts,
                        value: value.clone(),
                    },
                ));
            }
            WalRecord::Put {
                tx_id,
                row_id,
                value,
            } => {
                let tx = pending.get_mut(tx_id).ok_or_else(|| {
                    WalError::Corrupt(format!("PUT without BEGIN for tx {}", tx_id.0))
                })?;
                tx.mutations.push((*row_id, Mutation::Put(value.clone())));
            }
            WalRecord::Delete { tx_id, row_id } => {
                let tx = pending.get_mut(tx_id).ok_or_else(|| {
                    WalError::Corrupt(format!("DELETE without BEGIN for tx {}", tx_id.0))
                })?;
                tx.mutations.push((*row_id, Mutation::Delete));
            }
            WalRecord::Abort { tx_id } => {
                if pending.remove(tx_id).is_none() {
                    return Err(WalError::Corrupt(format!(
                        "ABORT without BEGIN for tx {}",
                        tx_id.0
                    )));
                }
                finished_ids.insert(*tx_id);
            }
            WalRecord::Commit { tx_id, commit_ts } => {
                let tx = pending.remove(tx_id).ok_or_else(|| {
                    WalError::Corrupt(format!("COMMIT without BEGIN for tx {}", tx_id.0))
                })?;
                if !finished_ids.insert(*tx_id) {
                    return Err(WalError::Corrupt(format!(
                        "duplicate COMMIT for tx {}",
                        tx_id.0
                    )));
                }
                committed.push((*lsn, *tx_id, *commit_ts, tx));
            }
        }
    }
    Ok(committed)
}
