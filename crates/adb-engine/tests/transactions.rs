//! Transactions module for the adb-engine crate.
//!
mod common;
use adb_core::RowId;
use adb_engine::{Database, DbError};
use common::{read_i64, row_with_i64};
use tempfile::tempdir;

/// Implements the `put_and_get` operation used by this subsystem.
#[test]
fn put_and_get() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(100));
    db.commit(tx).unwrap();
    assert_eq!(read_i64(&db.get(RowId(1)).unwrap().unwrap()), 100);
}

/// Implements the `transaction_reads_its_own_writes` operation used by this subsystem.
#[test]
fn transaction_reads_its_own_writes() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(123));
    assert_eq!(
        read_i64(&db.get_in_tx(&tx, RowId(1)).unwrap().unwrap()),
        123
    );
    assert!(db.get(RowId(1)).unwrap().is_none());
    db.rollback(tx).unwrap();
}

/// Writes the `write conflict is detected` value into the binary representation.
#[test]
fn write_write_conflict_is_detected() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut seed = db.begin();
    seed.put(RowId(1), row_with_i64(100));
    db.commit(seed).unwrap();
    let mut a = db.begin();
    let mut b = db.begin();
    a.put(RowId(1), row_with_i64(150));
    b.put(RowId(1), row_with_i64(200));
    db.commit(a).unwrap();
    assert!(matches!(db.commit(b), Err(DbError::TransactionConflict)));
}
