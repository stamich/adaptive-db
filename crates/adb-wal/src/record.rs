//! Records of the canonical log.
use adb_core::{CommitTs, Row, RowId, TxId};
use serde::{Deserialize, Serialize};

/// One log record. The bincode encoding is the persistent log format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WalRecord {
    /// Starts a transaction's run of records.
    Begin {
        /// Transaction.
        tx_id: TxId,
        /// Snapshot the transaction read at.
        snapshot_ts: CommitTs,
    },
    /// Before-image of a row the transaction overwrites. Makes the version projection and the
    /// change feed's `before` value derivable from the log alone.
    Version {
        /// Transaction.
        tx_id: TxId,
        /// Row.
        row_id: RowId,
        /// Commit that produced the before-image.
        begin_ts: CommitTs,
        /// Commit that replaces it (this transaction's commit).
        end_ts: CommitTs,
        /// Before-image (`None` when it was a tombstone).
        value: Option<Row>,
    },
    /// New row content.
    Put {
        /// Transaction.
        tx_id: TxId,
        /// Row.
        row_id: RowId,
        /// After-image.
        value: Row,
    },
    /// Row deletion.
    Delete {
        /// Transaction.
        tx_id: TxId,
        /// Row.
        row_id: RowId,
    },
    /// Commit point of the transaction.
    Commit {
        /// Transaction.
        tx_id: TxId,
        /// Commit timestamp.
        commit_ts: CommitTs,
    },
    /// Explicit abort (reserved; the engine never logs aborted transactions today).
    Abort {
        /// Transaction.
        tx_id: TxId,
    },
}
