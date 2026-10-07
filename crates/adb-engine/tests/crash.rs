//! Crash, recovery and corruption-repair scenarios.
//!
//! A "crash" is simulated by dropping the database without `close()`: nothing after the last
//! checkpoint has reached the projection files (no-steal), so reopen must replay the log.
mod common;

use std::fs;

use adb_core::RowId;
use adb_engine::{Database, DatabaseOptions, DbError};
use adb_storage::CheckpointStore;
use tempfile::tempdir;

use common::{put, value};

fn small_checkpoints() -> DatabaseOptions {
    DatabaseOptions {
        checkpoint_dirty_pages: 8,
        ..DatabaseOptions::default()
    }
}

#[test]
fn commits_after_the_last_checkpoint_are_replayed() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId(1), 1);
        db.checkpoint().unwrap();
        put(&db, RowId(1), 2);
        put(&db, RowId(2), 3);
    } // crash
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(value(&db, RowId(1)), Some(2));
    assert_eq!(value(&db, RowId(2)), Some(3));
    assert_eq!(db.history(RowId(1)).unwrap().len(), 2);
    assert!(db.verify().unwrap().is_ok());
}

/// Recovery starts at the checkpoint, not at the beginning of the log.
#[test]
fn checkpoint_moves_the_replay_start_forward() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for i in 0..10 {
        put(&db, RowId(i), i as i64);
    }
    db.checkpoint().unwrap();
    let checkpoint = CheckpointStore::new(dir.path().join("checkpoint.meta"))
        .load()
        .unwrap();
    assert!(checkpoint.replay_from.0 > 0);
    assert_eq!(checkpoint.last_commit_ts, db.latest_committed_ts());
}

/// Many automatic checkpoints interleaved with commits, then a crash, then reopen twice.
#[test]
fn automatic_checkpoints_and_repeated_recovery_preserve_every_commit() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open_with(dir.path(), small_checkpoints()).unwrap();
        for round in 0..20 {
            for row in 0..200u128 {
                put(&db, RowId(row), round * 1000 + row as i64);
            }
        }
    }
    for _ in 0..2 {
        let db = Database::open_with(dir.path(), small_checkpoints()).unwrap();
        for row in 0..200u128 {
            assert_eq!(value(&db, RowId(row)), Some(19_000 + row as i64));
        }
        assert_eq!(db.history(RowId(7)).unwrap().len(), 20);
        assert!(db.verify().unwrap().is_ok());
    }
}

/// The log is the source of truth and is never pruned by the engine.
#[test]
fn checkpoints_never_delete_log_segments() {
    let dir = tempdir().unwrap();
    let options = DatabaseOptions {
        log_segment_bytes: 4096,
        ..small_checkpoints()
    };
    let db = Database::open_with(dir.path(), options).unwrap();
    for i in 0..500u128 {
        put(&db, RowId(i), 0);
    }
    db.checkpoint().unwrap();
    assert_eq!(
        adb_wal::earliest_lsn(db.wal_dir()).unwrap(),
        Some(adb_core::Lsn(0))
    );
}

/// A damaged projection page is detected (here already at open, while the free-space map is
/// built), reported as corruption, and repaired by rebuilding the projections from the log.
#[test]
fn corrupted_projection_is_rebuilt_from_the_log() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        for i in 0..300u128 {
            put(&db, RowId(i), i as i64);
        }
        put(&db, RowId(5), 555);
        db.close().unwrap();
    }
    let heap = dir.path().join("current").join("current.heap");
    let mut bytes = fs::read(&heap).unwrap();
    for byte in &mut bytes[100..200] {
        *byte ^= 0xFF;
    }
    fs::write(&heap, bytes).unwrap();

    let error = match Database::open(dir.path()) {
        Ok(db) => db.get(RowId(0)).unwrap_err(),
        Err(error) => error,
    };
    assert!(error.is_corruption(), "{error}");

    let db = Database::rebuild_projections(dir.path(), DatabaseOptions::default()).unwrap();
    for i in 0..300u128 {
        let expected = if i == 5 { 555 } else { i as i64 };
        assert_eq!(value(&db, RowId(i)), Some(expected));
    }
    assert_eq!(db.history(RowId(5)).unwrap().len(), 2);
}

/// A failure after the commit started writing to the log poisons the instance; reopening
/// recovers a consistent state. The failure is injected by making log rotation impossible.
#[test]
fn failure_inside_commit_poisons_until_reopen() {
    let dir = tempdir().unwrap();
    let options = DatabaseOptions {
        log_segment_bytes: 1024,
        ..DatabaseOptions::default()
    };
    let obstacle = dir.path().join("wal").join("0000000000000001.wal");
    let committed;
    {
        let db = Database::open_with(dir.path(), options).unwrap();
        put(&db, RowId(1), 1);
        fs::create_dir_all(&obstacle).unwrap(); // the next rotation cannot create its file
        let mut last = Ok(adb_core::CommitTs(0));
        let mut i = 2;
        while last.is_ok() {
            let mut tx = db.begin();
            tx.put(RowId(i), common::row_with_i64(i as i64));
            last = db.commit(tx);
            i += 1;
        }
        committed = i - 2;
        assert!(matches!(last, Err(DbError::CommitOutcomeUnknown(_))));
        assert!(db.is_poisoned());
        assert!(matches!(db.get(RowId(1)), Err(DbError::Poisoned(_))));
        assert!(matches!(db.commit(db.begin()), Err(DbError::Poisoned(_))));
    }
    fs::remove_dir(&obstacle).unwrap();
    let db = Database::open_with(dir.path(), options).unwrap();
    for row in 1..=committed {
        assert_eq!(value(&db, RowId(row)), Some(row as i64), "row {row}");
    }
    assert_eq!(
        value(&db, RowId(committed + 1)),
        None,
        "failed commit is absent"
    );
    put(&db, RowId(1000), 1);
}

/// A checkpoint that dies after its journal commit point is completed by the next open.
/// The failure is injected by blocking the checkpoint record's temp file.
#[test]
fn interrupted_checkpoint_is_completed_on_open() {
    let dir = tempdir().unwrap();
    let obstacle = dir.path().join("checkpoint.meta.tmp");
    {
        let db = Database::open(dir.path()).unwrap();
        for i in 0..50u128 {
            put(&db, RowId(i), i as i64);
        }
        fs::create_dir(&obstacle).unwrap();
        assert!(db.checkpoint().is_err());
        assert!(db.is_poisoned());
        assert!(dir.path().join("checkpoint.journal").exists());
    }
    fs::remove_dir(&obstacle).unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert!(!dir.path().join("checkpoint.journal").exists());
    for i in 0..50u128 {
        assert_eq!(value(&db, RowId(i)), Some(i as i64));
    }
    assert!(db.verify().unwrap().is_ok());
}
