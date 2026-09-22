//! Recovery integration tests for durable current state, MVCC history, and crash tails.

mod common;

use std::{fs::OpenOptions, io::Write};

use adb_core::RowId;
use adb_engine::Database;
use tempfile::tempdir;

use common::{read_i64, row_with_i64};

/// Verifies that a committed current row survives a full database reopen.
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

/// Verifies that historical versions survive restart through the persistent Version Store.
#[test]
fn historical_versions_survive_restart_via_persistent_version_store() {
    let dir = tempdir().unwrap();
    let (t1, t2);
    {
        let db = Database::open(dir.path()).unwrap();
        let mut first = db.begin();
        first.put(RowId(1), row_with_i64(100));
        t1 = db.commit(first).unwrap();

        let mut second = db.begin();
        second.put(RowId(1), row_with_i64(200));
        t2 = db.commit(second).unwrap();
    }
    {
        let db = Database::open(dir.path()).unwrap();
        assert_eq!(read_i64(&db.get_at(RowId(1), t1).unwrap().unwrap()), 100);
        assert_eq!(read_i64(&db.get_at(RowId(1), t2).unwrap().unwrap()), 200);
    }
}

/// Verifies that an incomplete suffix in the newest WAL segment is ignored and normalized on reopen.
#[test]
fn truncated_wal_tail_is_ignored() {
    let dir = tempdir().unwrap();
    let wal_path;
    {
        let db = Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.put(RowId(1), row_with_i64(100));
        db.commit(tx).unwrap();
        wal_path = db.wal_dir().to_path_buf();
    }

    let mut segments = std::fs::read_dir(&wal_path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("wal"))
        .collect::<Vec<_>>();
    segments.sort();

    let tail = segments.last().unwrap();
    let mut file = OpenOptions::new().append(true).open(tail).unwrap();
    file.write_all(&[0x57, 0x42]).unwrap();
    file.flush().unwrap();

    let db = Database::open(dir.path()).unwrap();
    assert_eq!(read_i64(&db.get(RowId(1)).unwrap().unwrap()), 100);
}
