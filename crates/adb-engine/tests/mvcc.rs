//! Mvcc module for the adb-engine crate.
//!
mod common;

use adb_core::RowId;
use adb_engine::Database;
use tempfile::tempdir;

use common::{read_i64, row_with_i64};

/// Implements the `snapshot_read_is_repeatable` operation used by this subsystem.
#[test]
fn snapshot_read_is_repeatable() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut seed = db.begin();
    seed.put(RowId(1), row_with_i64(100));
    db.commit(seed).unwrap();

    let tx1 = db.begin();

    let mut tx2 = db.begin();
    tx2.put(RowId(1), row_with_i64(200));
    db.commit(tx2).unwrap();

    let from_snapshot = db.get_in_tx(&tx1, RowId(1)).unwrap().unwrap();
    assert_eq!(read_i64(&from_snapshot), 100);

    let current = db.get(RowId(1)).unwrap();
    assert_eq!(read_i64(&current), 200);
}

/// Implements the `historical_versions_are_queryable` operation used by this subsystem.
#[test]
fn historical_versions_are_queryable() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut tx1 = db.begin();
    tx1.put(RowId(1), row_with_i64(100));
    let ts1 = db.commit(tx1).unwrap();

    let mut tx2 = db.begin();
    tx2.put(RowId(1), row_with_i64(200));
    let ts2 = db.commit(tx2).unwrap();

    let mut tx3 = db.begin();
    tx3.put(RowId(1), row_with_i64(300));
    let ts3 = db.commit(tx3).unwrap();

    assert_eq!(read_i64(&db.get_at(RowId(1), ts1).unwrap()), 100);
    assert_eq!(read_i64(&db.get_at(RowId(1), ts2).unwrap()), 200);
    assert_eq!(read_i64(&db.get_at(RowId(1), ts3).unwrap()), 300);
}
