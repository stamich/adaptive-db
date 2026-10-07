//! Commit-time validation (backward optimistic concurrency control).
use adb_core::{CommitTs, RowId};

use crate::{IsolationLevel, Transaction};

/// Why a transaction cannot commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// A row this transaction writes was committed by someone else after its snapshot.
    WriteWrite(RowId),
    /// A row this transaction read was changed after its snapshot (serializable only).
    ReadWrite(RowId),
}

/// Validates `tx` against the latest commit timestamp of each row.
///
/// `latest_commit` must return the commit timestamp of the newest committed state of a row
/// (including delete tombstones), or `None` if the row never existed or was vacuumed.
/// The caller must hold the commit lock so the answer cannot change before the commit.
pub fn validate<E>(
    tx: &Transaction,
    mut latest_commit: impl FnMut(RowId) -> Result<Option<CommitTs>, E>,
) -> Result<Result<(), Conflict>, E> {
    let changed_since_snapshot = |ts: Option<CommitTs>| ts.is_some_and(|ts| ts > tx.snapshot_ts());

    for row_id in tx.writes().keys() {
        if changed_since_snapshot(latest_commit(*row_id)?) {
            return Ok(Err(Conflict::WriteWrite(*row_id)));
        }
    }
    if tx.isolation() == IsolationLevel::Serializable {
        for row_id in tx.reads() {
            if !tx.writes().contains_key(row_id) && changed_since_snapshot(latest_commit(*row_id)?)
            {
                return Ok(Err(Conflict::ReadWrite(*row_id)));
            }
        }
    }
    Ok(Ok(()))
}
