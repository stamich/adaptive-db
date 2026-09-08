//! Recovery module for the adb-engine crate.
//!
mod common;
use adb_core::RowId;
use adb_engine::Database;
use common::{read_i64, row_with_i64};
use std::{fs::OpenOptions, io::Write};
use tempfile::tempdir;

/// Implements the `committed_transaction_survives_restart_using_persistent_current_store` operation used by this subsystem.
#[test]
fn committed_transaction_survives_restart_using_persistent_current_store() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(100));
        db.commit(tx).unwrap();
    }
    {
        let db = Database::open(dir.path()).unwrap();
        assert_eq!(read_i64(&db.get(RowId(1)).unwrap().unwrap()), 100);
    }
}

/// Implements the `historical_versions_survive_restart_via_wal_reconstruction` operation used by this subsystem.
#[test]
fn historical_versions_survive_restart_via_wal_reconstruction() {
    let dir = tempdir().unwrap();
    let (t1, t2);
    {
        let db = Database::open(dir.path()).unwrap();
        let mut a = db.begin();
        a.put(RowId(1), row_with_i64(100));
        t1 = db.commit(a).unwrap();
        let mut b = db.begin();
        b.put(RowId(1), row_with_i64(200));
        t2 = db.commit(b).unwrap();
    }
    {
        let db = Database::open(dir.path()).unwrap();
        assert_eq!(read_i64(&db.get_at(RowId(1), t1).unwrap().unwrap()), 100);
        assert_eq!(read_i64(&db.get_at(RowId(1), t2).unwrap().unwrap()), 200);
    }
}

/// Implements the `truncated_wal_tail_is_ignored` operation used by this subsystem.
#[test]
fn truncated_wal_tail_is_ignored() {
    let dir = tempdir().unwrap();
    let wal_path;
    {
        let db = Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(100));
        db.commit(tx).unwrap();
        wal_path = db.wal_path().to_path_buf();
    }
    let mut f = OpenOptions::new().append(true).open(wal_path).unwrap();
    f.write_all(&[0x57, 0x42]).unwrap();
    f.flush().unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(read_i64(&db.get(RowId(1)).unwrap().unwrap()), 100);
}
