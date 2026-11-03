//! The change log is derived data: whatever happens to it, opening the database brings it back in
//! line with the canonical log.
mod common;

use std::path::{Path, PathBuf};

use adb_change_log::{ChangeEnvelope, ChangeLog, ChangePayload, DEFAULT_SEGMENT_BYTES};
use adb_core::{CommitSeq, RowId, ValueType};
use adb_engine::{Database, EntitySpec, NewField};
use common::put;
use tempfile::tempdir;

/// `CREATE TABLE t (id BIGINT PRIMARY KEY)`.
fn table() -> EntitySpec {
    EntitySpec {
        name: "t".into(),
        fields: vec![NewField {
            name: "id".into(),
            value_type: ValueType::Int64,
            nullable: false,
            default: None,
            references: None,
            id: None,
        }],
        primary_key: "id".into(),
        entity_id: None,
    }
}

/// Directory of the change log inside a database directory.
fn log_dir(db_dir: &Path) -> PathBuf {
    db_dir.join("change-log")
}

/// Commits 10 data, 11 DDL, 12 data and closes cleanly; returns what the change log held.
fn populate(dir: &Path) -> Vec<ChangeEnvelope> {
    let db = Database::open(dir).unwrap();
    for i in 1..=10 {
        put(&db, RowId(i), i as i64);
    }
    db.create_entity(table()).unwrap();
    put(&db, RowId(12), 12);
    let envelopes = db.change_log().read_after(CommitSeq::ZERO, 100).unwrap();
    db.close().unwrap();
    envelopes
}

/// What the reopened database's change log holds.
fn reopened(dir: &Path) -> Vec<ChangeEnvelope> {
    Database::open(dir)
        .unwrap()
        .change_log()
        .read_after(CommitSeq::ZERO, 100)
        .unwrap()
}

/// A database without a change log (written by an older engine) gets one rebuilt from the WAL,
/// identical to what the commit path produced, with DML and DDL in one order.
#[test]
fn a_missing_change_log_is_rebuilt_from_the_wal() {
    let dir = tempdir().unwrap();
    let original = populate(dir.path());
    std::fs::remove_dir_all(log_dir(dir.path())).unwrap();
    let rebuilt = reopened(dir.path());
    assert_eq!(rebuilt, original);
    assert_eq!(rebuilt.len(), 12);
    assert!(matches!(rebuilt[10].payload, ChangePayload::Schema { .. }));
    assert!(matches!(rebuilt[11].payload, ChangePayload::Data { .. }));
}

/// A lost tail (commits after the last durable change-log byte) is rebuilt from the WAL tail.
#[test]
fn a_lost_change_log_tail_is_rebuilt() {
    let dir = tempdir().unwrap();
    let original = populate(dir.path());
    // Keep only the first 4 envelopes.
    let log = ChangeLog::open(&log_dir(dir.path()), DEFAULT_SEGMENT_BYTES).unwrap();
    log.truncate_after(CommitSeq(4)).unwrap();
    assert_eq!(log.head(), Some(CommitSeq(4)));
    drop(log);
    assert_eq!(reopened(dir.path()), original);
}

/// A torn last frame is cut and rebuilt; damage in the middle discards the log, which is then
/// rebuilt in full.
#[test]
fn damaged_change_logs_are_rebuilt() {
    let dir = tempdir().unwrap();
    let original = populate(dir.path());
    let segment = std::fs::read_dir(log_dir(dir.path()))
        .unwrap()
        .map(|e| e.unwrap().path())
        .next()
        .unwrap();
    let bytes = std::fs::read(&segment).unwrap();
    std::fs::write(&segment, &bytes[..bytes.len() - 5]).unwrap();
    assert_eq!(reopened(dir.path()), original);

    let mut damaged = std::fs::read(&segment).unwrap();
    damaged[12 + 8 + 3] ^= 0xff;
    std::fs::write(&segment, &damaged).unwrap();
    assert_eq!(reopened(dir.path()), original);
}

/// Envelopes of commits the WAL no longer has (a crash took the WAL tail) are removed, and the
/// numbers are reused by the next commits with their real content.
#[test]
fn surplus_envelopes_are_removed() {
    let dir = tempdir().unwrap();
    let original = populate(dir.path());
    // Forge an envelope for a commit 13 that the WAL never saw.
    let log = ChangeLog::open(&log_dir(dir.path()), DEFAULT_SEGMENT_BYTES).unwrap();
    let mut forged = original.last().unwrap().clone();
    forged.commit_seq = CommitSeq(13);
    log.append(&forged).unwrap();
    log.sync().unwrap();
    drop(log);

    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.change_log().head(), Some(CommitSeq(12)));
    put(&db, RowId(99), 99);
    let all = db.change_log().read_after(CommitSeq::ZERO, 100).unwrap();
    assert_eq!(all.len(), 13);
    assert_eq!(all[12].commit_seq, CommitSeq(13));
    assert_ne!(all[12], forged);
}

/// An intact, current change log is left alone by a reopen.
#[test]
fn a_current_change_log_is_untouched() {
    let dir = tempdir().unwrap();
    let original = populate(dir.path());
    assert_eq!(reopened(dir.path()), original);
    assert_eq!(reopened(dir.path()), original);
}

/// A crash without a clean close (the database is dropped, nothing checkpointed) recovers
/// both the stores and the change log from the WAL.
#[test]
fn recovery_after_a_crash_without_checkpoint() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        for i in 1..=5 {
            put(&db, RowId(i), i as i64);
        }
        // No close: dropped without a checkpoint.
    }
    // Lose the unsynced change-log tail entirely.
    std::fs::remove_dir_all(log_dir(dir.path())).unwrap();
    let envelopes = reopened(dir.path());
    let seqs: Vec<u64> = envelopes.iter().map(|e| e.commit_seq.0).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
}
