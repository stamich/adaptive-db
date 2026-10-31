//! Records of the canonical log.
use adb_core::{CommitTs, Row, RowId, SchemaEpoch, TxId};
pub use adb_schema::CatalogEvent;
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
    /// A catalog change (data format 3, Milestone 2.4.0). Schema history lives in the canonical
    /// log, so DDL and the data commits that depend on it have one order, recovery and
    /// `rebuild_projections` restore the schema registry like every other projection, and the
    /// change feed shows schema changes where they happened. Appended last so the encoding of
    /// every earlier record kind is unchanged.
    Catalog {
        /// Transaction.
        tx_id: TxId,
        /// Catalog epoch after this change.
        epoch: SchemaEpoch,
        /// The change.
        event: CatalogEvent,
    },
}
