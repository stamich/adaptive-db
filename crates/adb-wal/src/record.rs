//! Record module for the adb-wal crate.
//!
use adb_core::{CommitTs, Row, RowId, TxId};
use serde::{Deserialize, Serialize};

/// Enumerates the supported `WalRecord` variants used by this subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalRecord {
    Begin {
        tx_id: TxId,
        snapshot_ts: CommitTs,
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
