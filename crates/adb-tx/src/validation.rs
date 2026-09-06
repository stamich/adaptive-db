//! Validation module for the adb-tx crate.
//!
use adb_storage::CurrentRecord;

use crate::Transaction;

/// Checks whether a transaction can update a row without violating snapshot write-conflict rules.
pub fn validate_write(
    current: Option<&CurrentRecord>,
    tx: &Transaction,
) -> bool {
    match current {
        Some(record) => record.commit_ts <= tx.snapshot_ts(),
        None => true,
    }
}
