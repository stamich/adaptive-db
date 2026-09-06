//! Transactions module for the adb-engine crate.
//!
mod common;

use adb_core::RowId;
use adb_engine::{Database, DbError};
use tempfile::tempdir;

use common::{read_i64, row_with_i64};

/// Implements the `put_and_get` operation used by this subsystem.
#[test]
fn put_and_get() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(100));
    db.commit(tx).unwrap();

    let row = db.get(RowId(1)).unwrap();
    assert_eq!(read_i64(&row), 100);
}

/// Implements the `transaction_reads_its_own_writes` operation used by this subsystem.
#[test]
fn transaction_reads_its_own_writes() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(123));

    let row = db.get_in_tx(&tx, RowId(1)).unwrap().unwrap();
    assert_eq!(read_i64(&row), 123);
    assert!(db.get(RowId(1)).is_none());

    db.rollback(tx).unwrap();
    assert!(db.get(RowId(1)).is_none());
}

/// Implements the `delete_removes_current_value` operation used by this subsystem.
#[test]
fn delete_removes_current_value() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut tx1 = db.begin();
    tx1.put(RowId(1), row_with_i64(100));
    db.commit(tx1).unwrap();

    let mut tx2 = db.begin();
    tx2.delete(RowId(1));
    db.commit(tx2).unwrap();

    assert!(db.get(RowId(1)).is_none());
}

/// Writes the `write conflict is detected` value into the binary representation.
#[test]
fn write_write_conflict_is_detected() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut seed = db.begin();
    seed.put(RowId(1), row_with_i64(100));
    db.commit(seed).unwrap();

    let mut tx1 = db.begin();
    let mut tx2 = db.begin();

    tx1.put(RowId(1), row_with_i64(150));
    tx2.put(RowId(1), row_with_i64(200));

    db.commit(tx1).unwrap();

    let result = db.commit(tx2);
    assert!(matches!(result, Err(DbError::TransactionConflict)));
}


/// Implements the `new_snapshot_observes_only_published_commits` operation used by this subsystem.
#[test]
fn new_snapshot_observes_only_published_commits() {
    use adb_tx::TransactionManager;

    let mut tm = TransactionManager::default();

    let before = tm.begin();
    assert_eq!(before.snapshot_ts().0, 0);

    let reserved = tm.allocate_commit_ts();

    // Reservation alone is not a commit publication.
    let during = tm.begin();
    assert_eq!(during.snapshot_ts().0, 0);

    tm.publish_commit(reserved);

    let after = tm.begin();
    assert_eq!(after.snapshot_ts(), reserved);
}
