use adb_storage::CurrentRecord;

use crate::Transaction;

pub fn validate_write(
    current: Option<&CurrentRecord>,
    tx: &Transaction,
) -> bool {
    match current {
        Some(record) => record.commit_ts <= tx.snapshot_ts(),
        None => true,
    }
}
