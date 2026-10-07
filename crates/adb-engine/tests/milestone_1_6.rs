//! Persistent MVCC history across restarts.
mod common;

use adb_core::RowId;
use adb_engine::Database;
use tempfile::tempdir;

use common::{read_i64, row_with_i64};

/// Every historical version stays readable across two restarts.
#[test]
fn persistent_history_survives_restart() {
    let dir = tempdir().unwrap();

    let (ts1, ts2, ts3, ts4);

    {
        let db = Database::open(dir.path()).unwrap();

        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(100));
        ts1 = db.commit(tx).unwrap();

        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(200));
        ts2 = db.commit(tx).unwrap();

        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(300));
        ts3 = db.commit(tx).unwrap();
    }

    {
        let db = Database::open(dir.path()).unwrap();

        assert_eq!(read_i64(&db.get_at(RowId(1), ts1).unwrap().unwrap()), 100);
        assert_eq!(read_i64(&db.get_at(RowId(1), ts2).unwrap().unwrap()), 200);
        assert_eq!(read_i64(&db.get_at(RowId(1), ts3).unwrap().unwrap()), 300);

        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(400));
        ts4 = db.commit(tx).unwrap();
    }

    {
        let db = Database::open(dir.path()).unwrap();

        assert_eq!(read_i64(&db.get_at(RowId(1), ts1).unwrap().unwrap()), 100);
        assert_eq!(read_i64(&db.get_at(RowId(1), ts2).unwrap().unwrap()), 200);
        assert_eq!(read_i64(&db.get_at(RowId(1), ts3).unwrap().unwrap()), 300);
        assert_eq!(read_i64(&db.get_at(RowId(1), ts4).unwrap().unwrap()), 400);

        let history = db.history(RowId(1)).unwrap();
        assert_eq!(history.len(), 4);

        let report = db.verify().unwrap();
        assert!(report.is_ok(), "{:?}", report.errors);
    }
}

/// A delete hides the row from later snapshots only.
#[test]
fn delete_is_temporal() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let mut tx = db.begin();
    tx.put(RowId(10), row_with_i64(100));
    let created = db.commit(tx).unwrap();

    let mut tx = db.begin();
    tx.delete(RowId(10));
    let deleted = db.commit(tx).unwrap();

    assert_eq!(
        read_i64(&db.get_at(RowId(10), created).unwrap().unwrap()),
        100
    );

    assert!(db.get_at(RowId(10), deleted).unwrap().is_none());
}
