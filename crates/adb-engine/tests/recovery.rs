mod common;

use std::{
    fs::OpenOptions,
    io::Write,
};

use adb_core::RowId;
use adb_engine::Database;
use tempfile::tempdir;

use common::{read_i64, row_with_i64};

#[test]
fn committed_transaction_survives_restart() {
    let dir = tempdir().unwrap();

    {
        let db = Database::open(dir.path()).unwrap();

        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(100));
        db.commit(tx).unwrap();
    }

    {
        let db = Database::open(dir.path()).unwrap();
        let row = db.get(RowId(1)).unwrap();
        assert_eq!(read_i64(&row), 100);
    }
}

#[test]
fn multiple_versions_survive_restart() {
    let dir = tempdir().unwrap();

    let (ts1, ts2);

    {
        let db = Database::open(dir.path()).unwrap();

        let mut tx1 = db.begin();
        tx1.put(RowId(1), row_with_i64(100));
        ts1 = db.commit(tx1).unwrap();

        let mut tx2 = db.begin();
        tx2.put(RowId(1), row_with_i64(200));
        ts2 = db.commit(tx2).unwrap();
    }

    {
        let db = Database::open(dir.path()).unwrap();

        assert_eq!(
            read_i64(&db.get_at(RowId(1), ts1).unwrap()),
            100
        );
        assert_eq!(
            read_i64(&db.get_at(RowId(1), ts2).unwrap()),
            200
        );
    }
}

#[test]
fn truncated_wal_tail_is_ignored() {
    let dir = tempdir().unwrap();

    let wal_path = {
        let db = Database::open(dir.path()).unwrap();

        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(100));
        db.commit(tx).unwrap();

        db.wal_path().to_path_buf()
    };

    // Simulate a process crash during the next WAL record.
    let mut file = OpenOptions::new()
        .append(true)
        .open(&wal_path)
        .unwrap();

    file.write_all(&[0x57, 0x42]).unwrap();
    file.flush().unwrap();

    let db = Database::open(dir.path()).unwrap();
    assert_eq!(read_i64(&db.get(RowId(1)).unwrap()), 100);
}
