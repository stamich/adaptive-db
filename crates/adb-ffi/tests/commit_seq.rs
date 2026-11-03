//! Commit sequence numbers over the C ABI (ABI 8): the latest number, the `*_commit` mutations
//! and the change feed agree, and the old entry points keep working.
use std::ptr;

use adb_ffi::*;
use serde_json::Value;
use tempfile::tempdir;

/// Raw database handle type.
type Db = *mut adb_ffi::handle::AdbDatabaseHandle;

/// Opens a database through `adb_open`.
fn open(dir: &std::path::Path) -> Db {
    let path = dir.to_string_lossy().as_bytes().to_vec();
    let mut db = ptr::null_mut();
    assert_eq!(adb_open(path.as_ptr(), path.len(), &mut db), AdbStatus::Ok);
    db
}

/// The newest published sequence number.
fn latest(db: Db) -> u64 {
    let mut seq = u64::MAX;
    assert_eq!(adb_latest_commit_seq(db, &mut seq), AdbStatus::Ok);
    seq
}

/// A row with field 1 set to `value`.
fn row(value: i64) -> Vec<u8> {
    format!(r#"{{"fields":{{"1":{{"Int64":{value}}}}}}}"#).into_bytes()
}

/// The commit functions report consecutive numbers; the old ones still work and consume numbers
/// too; a null `out_commit_seq` is accepted.
#[test]
fn mutations_report_their_commit_sequence() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    assert_eq!(latest(db), 0);

    let (mut ts, mut seq) = (0u64, 0u64);
    let r = row(1);
    assert_eq!(
        adb_insert_row_commit_json(db, 5, 1, r.as_ptr(), r.len(), &mut ts, &mut seq),
        AdbStatus::Ok
    );
    assert_eq!((seq, latest(db)), (1, 1));
    assert!(ts > 0);

    let update = br#"{"1":{"Int64":2}}"#;
    assert_eq!(
        adb_update_fields_commit_json(db, 5, 1, update.as_ptr(), update.len(), &mut ts, &mut seq),
        AdbStatus::Ok
    );
    assert_eq!(seq, 2);

    // The pre-ABI-8 entry points consume numbers like any other commit.
    let r2 = row(7);
    assert_eq!(
        adb_insert_row_json(db, 5, 2, r2.as_ptr(), r2.len(), &mut ts),
        AdbStatus::Ok
    );
    assert_eq!(latest(db), 3);
    assert_eq!(
        adb_insert_row_commit_json(db, 5, 3, r2.as_ptr(), r2.len(), &mut ts, ptr::null_mut()),
        AdbStatus::Ok
    );
    assert_eq!(latest(db), 4);

    assert_eq!(
        adb_delete_row_commit(db, 5, 1, &mut ts, &mut seq),
        AdbStatus::Ok
    );
    assert_eq!(seq, 5);
    // A failed mutation leaves the output at 0 and consumes nothing.
    seq = 99;
    assert_eq!(
        adb_delete_row_commit(db, 5, 1, &mut ts, &mut seq),
        AdbStatus::NotFound
    );
    assert_eq!(seq, 0);
    assert_eq!(latest(db), 5);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Argument checking: null handle, null output.
#[test]
fn argument_validation() {
    let mut seq = 7u64;
    assert_eq!(
        adb_latest_commit_seq(ptr::null_mut(), &mut seq),
        AdbStatus::InvalidArgument
    );
    assert_eq!(seq, 0, "the output is initialised before anything fails");
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    assert_eq!(
        adb_latest_commit_seq(db, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// The number survives a reopen and the change feed names it for every event.
#[test]
fn the_change_feed_carries_the_sequence() {
    let dir = tempdir().unwrap();
    {
        let db = open(dir.path());
        let r = row(1);
        let mut ts = 0;
        for pk in 1..=3 {
            assert_eq!(
                adb_insert_row_json(db, 5, pk, r.as_ptr(), r.len(), &mut ts),
                AdbStatus::Ok
            );
        }
        assert_eq!(adb_close(db), AdbStatus::Ok);
    }
    let db = open(dir.path());
    assert_eq!(latest(db), 3);
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_read_changes_json(db, 0, 100, ptr::null(), 0, &mut out),
        AdbStatus::Ok
    );
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(out), adb_batch_len(out)) }.to_vec();
    assert_eq!(adb_batch_release(out), AdbStatus::Ok);
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    let seqs: Vec<u64> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["commit_seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3]);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
