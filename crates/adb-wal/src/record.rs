//! Module `record` for crate `adb-wal`.
use adb_core::{CommitTs, Row, RowId, TxId};
use serde::{Deserialize, Serialize};

/// Enumerates `WalRecord` alternatives used by this subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalRecord {
    Begin {
        tx_id: TxId,
        snapshot_ts: CommitTs,
    },

    /// Explicit historical before-image.
    /// This makes persistent VersionStore replay independent of CurrentStore
    /// flush ordering.
    Version {
        tx_id: TxId,
        row_id: RowId,
        begin_ts: CommitTs,
        end_ts: CommitTs,
        value: Option<Row>,
    },

    Put {
        tx_id: TxId,
        row_id: RowId,
        value: Row,
    },

    Delete {
        tx_id: TxId,
        row_id: RowId,
    },

    Commit {
        tx_id: TxId,
        commit_ts: CommitTs,
    },

    Abort {
        tx_id: TxId,
    },
}
