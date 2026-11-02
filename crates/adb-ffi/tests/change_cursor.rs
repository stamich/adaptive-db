//! Change-log cursors over the C ABI (ABI 8): streaming, limits, end of stream, truncation and
//! zeroed outputs on every failure.
use std::ptr;

use adb_ffi::{handle::AdbChangeCursorHandle, *};
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

/// Inserts `count` rows of entity 5, one commit each.
fn insert(db: Db, count: u64) {
    let row = br#"{"fields":{"1":{"Int64":1}}}"#;
    for pk in 1..=count {
        let mut ts = 0;
        assert_eq!(
            adb_insert_row_json(db, 5, pk, row.as_ptr(), row.len(), &mut ts),
            AdbStatus::Ok
        );
    }
}

/// Opens a cursor after `seq`.
fn cursor(db: Db, seq: u64) -> *mut AdbChangeCursorHandle {
    let mut cursor = ptr::null_mut();
    assert_eq!(adb_change_cursor_open(db, seq, &mut cursor), AdbStatus::Ok);
    assert!(!cursor.is_null());
    cursor
}

/// Copies a JSON buffer out of its handle, releases it and parses it.
fn take_json(handle: *mut adb_ffi::handle::AdbBatchHandle) -> Value {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(handle), adb_batch_len(handle)) }
            .to_vec();
    assert_eq!(adb_batch_release(handle), AdbStatus::Ok);
    serde_json::from_slice(&bytes).unwrap()
}

/// Batches arrive in order within the commit limit, then the stream ends and picks up new
/// commits.
#[test]
fn streaming_in_batches() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    insert(db, 5);
    let c = cursor(db, 0);
    let mut seen = Vec::new();
    loop {
        let mut out = ptr::null_mut();
        let status = adb_change_cursor_next_batch(c, 2, 1 << 20, &mut out);
        if status == AdbStatus::EndOfStream {
            assert!(out.is_null());
            break;
        }
        assert_eq!(status, AdbStatus::Ok);
        let batch = take_json(out);
        let commits = batch["commits"].as_array().unwrap();
        assert!(commits.len() <= 2);
        for commit in commits {
            assert_eq!(commit["kind"], "data");
            assert_eq!(commit["rows"][0]["kind"], "insert");
            assert_eq!(commit["rows"][0]["entity_id"], 5);
            seen.push(
                commit["commit_seq"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap(),
            );
        }
        assert_eq!(
            batch["last_commit_seq"],
            commits.last().unwrap()["commit_seq"]
        );
    }
    assert_eq!(seen, [1, 2, 3, 4, 5]);
    let mut position = 0;
    assert_eq!(adb_change_cursor_position(c, &mut position), AdbStatus::Ok);
    assert_eq!(position, 5);

    insert_more(db);
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_change_cursor_next_batch(c, 10, 1 << 20, &mut out),
        AdbStatus::Ok
    );
    assert_eq!(
        take_json(out)["commits"][0]["rows"][0]["primary_key"],
        "100"
    );
    assert_eq!(adb_change_cursor_close(c), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Adds the row with primary key 100.
fn insert_more(db: Db) {
    let row = br#"{"fields":{"1":{"Int64":1}}}"#;
    let mut ts = 0;
    assert_eq!(
        adb_insert_row_json(db, 5, 100, row.as_ptr(), row.len(), &mut ts),
        AdbStatus::Ok
    );
}

/// A cursor starting inside the log skips earlier commits; DDL commits appear as schema commits.
#[test]
fn start_position_and_ddl() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    insert(db, 3);
    let spec = br#"{"name":"t","fields":[{"name":"id","value_type":"Int64","nullable":false}],"primary_key":"id"}"#;
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_create_entity_json(db, spec.as_ptr(), spec.len(), &mut out),
        AdbStatus::Ok
    );
    take_json(out);
    let c = cursor(db, 2);
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_change_cursor_next_batch(c, 10, 1 << 20, &mut out),
        AdbStatus::Ok
    );
    let batch = take_json(out);
    assert_eq!(batch["first_commit_seq"], "3");
    assert_eq!(batch["commits"][1]["kind"], "schema");
    assert_eq!(batch["commits"][1]["changes"][0]["kind"], "schema");
    assert_eq!(adb_change_cursor_close(c), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Argument validation: outputs are zeroed first, limits are enforced, nulls are refused.
#[test]
fn argument_validation() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    insert(db, 1);
    let mut bad: *mut AdbChangeCursorHandle = ptr::dangling_mut();
    assert_eq!(
        adb_change_cursor_open(ptr::null_mut(), 0, &mut bad),
        AdbStatus::InvalidArgument
    );
    assert!(bad.is_null());
    assert_eq!(
        adb_change_cursor_open(db, 0, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    let c = cursor(db, 0);
    let mut out: *mut adb_ffi::handle::AdbBatchHandle = ptr::dangling_mut();
    for (commits, bytes) in [(0, 10), (10_001, 10), (1, 0), (1, 65 * 1024 * 1024)] {
        out = ptr::dangling_mut();
        assert_eq!(
            adb_change_cursor_next_batch(c, commits, bytes, &mut out),
            AdbStatus::InvalidArgument,
            "{commits} {bytes}"
        );
        assert!(out.is_null());
    }
    assert_eq!(
        adb_change_cursor_next_batch(ptr::null_mut(), 1, 10, &mut out),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_change_cursor_next_batch(c, 1, 10, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    let mut position = 9;
    assert_eq!(
        adb_change_cursor_position(ptr::null_mut(), &mut position),
        AdbStatus::InvalidArgument
    );
    assert_eq!(position, 0);
    assert_eq!(
        adb_change_cursor_close(ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_change_cursor_close(c), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
