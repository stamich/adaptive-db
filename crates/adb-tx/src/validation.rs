//! Module `validation` for crate `adb-tx`.
use adb_storage::CurrentRecord;

use crate::Transaction;

/// Implements the `validate_write` operation used by this subsystem.
pub fn validate_write(current: Option<&CurrentRecord>, tx: &Transaction) -> bool {
    match current {
        Some(record) => record.commit_ts <= tx.snapshot_ts(),
        None => true,
    }
}
