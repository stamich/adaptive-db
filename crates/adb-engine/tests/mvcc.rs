//! Mvcc module for the adb-engine crate.
//!
mod common;
use adb_core::RowId;
use adb_engine::Database;
use common::{read_i64, row_with_i64};
use tempfile::tempdir;

/// Implements the `snapshot_read_is_repeatable` operation used by this subsystem.
#[test]
fn snapshot_read_is_repeatable() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut seed = db.begin();
    seed.put(RowId(1), row_with_i64(100));
    db.commit(seed).unwrap();
    let old = db.begin();
    let mut newer = db.begin();
    newer.put(RowId(1), row_with_i64(200));
    db.commit(newer).unwrap();
    assert_eq!(
        read_i64(&db.get_in_tx(&old, RowId(1)).unwrap().unwrap()),
        100
    );
    assert_eq!(read_i64(&db.get(RowId(1)).unwrap().unwrap()), 200);
}

/// Implements the `historical_versions_are_queryable` operation used by this subsystem.
#[test]
fn historical_versions_are_queryable() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut a = db.begin();
    a.put(RowId(1), row_with_i64(100));
    let t1 = db.commit(a).unwrap();
    let mut b = db.begin();
    b.put(RowId(1), row_with_i64(200));
    let t2 = db.commit(b).unwrap();
    let mut c = db.begin();
    c.put(RowId(1), row_with_i64(300));
    let t3 = db.commit(c).unwrap();
    assert_eq!(read_i64(&db.get_at(RowId(1), t1).unwrap().unwrap()), 100);
    assert_eq!(read_i64(&db.get_at(RowId(1), t2).unwrap().unwrap()), 200);
    assert_eq!(read_i64(&db.get_at(RowId(1), t3).unwrap().unwrap()), 300);
}
