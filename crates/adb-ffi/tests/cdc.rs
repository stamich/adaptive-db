//! End-to-end C ABI test of mutations, entity scans, change feed, offsets and maintenance.
use std::ptr;

use adb_ffi::*;
use serde_json::Value;
use tempfile::tempdir;

/// Opens a database through `adb_open`.
fn open(dir: &std::path::Path) -> *mut adb_ffi::handle::AdbDatabaseHandle {
    let path = dir.to_string_lossy().as_bytes().to_vec();
    let mut db = ptr::null_mut();
    assert_eq!(adb_open(path.as_ptr(), path.len(), &mut db), AdbStatus::Ok);
    db
}

/// Inserts a row with field 1 = `value` through `adb_insert_row_json`.
fn insert(db: *mut adb_ffi::handle::AdbDatabaseHandle, entity: u64, pk: u64, value: i64) {
    let row = format!(r#"{{"fields":{{"1":{{"Int64":{value}}}}}}}"#).into_bytes();
    let mut ts = 0;
    assert_eq!(
        adb_insert_row_json(db, entity, pk, row.as_ptr(), row.len(), &mut ts),
        AdbStatus::Ok
    );
    assert!(ts > 0);
}

/// Copies a JSON buffer out of its handle, releases it and parses it.
fn take_json(handle: *mut adb_ffi::handle::AdbBatchHandle) -> Value {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(handle), adb_batch_len(handle)) }
            .to_vec();
    assert_eq!(adb_batch_release(handle), AdbStatus::Ok);
    serde_json::from_slice(&bytes).unwrap()
}

/// Reads up to 100 change events after `cursor`, optionally filtered by entity.
fn read_changes(
    db: *mut adb_ffi::handle::AdbDatabaseHandle,
    cursor: u64,
    entities: &[u64],
) -> Value {
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_read_changes_json(db, cursor, 100, entities.as_ptr(), entities.len(), &mut out),
        AdbStatus::Ok
    );
    take_json(out)
}

/// Mutations, change feed, filters, offsets, vacuum and checkpoint work through the C ABI and survive a reopen.
#[test]
fn change_feed_and_offsets_over_the_c_abi() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    insert(db, 1, 10, 100);
    insert(db, 2, 20, 200);
    let assignments = br#"{"1":{"Int64":101}}"#;
    let mut ts = 0;
    assert_eq!(
        adb_update_fields_json(db, 1, 10, assignments.as_ptr(), assignments.len(), &mut ts),
        AdbStatus::Ok
    );
    assert_eq!(adb_delete_row(db, 2, 20, &mut ts), AdbStatus::Ok);

    let all = read_changes(db, 0, &[]);
    let kinds: Vec<_> = all["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["changes"][0]["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["insert", "insert", "update", "delete"]);
    let update = &all["events"][2]["changes"][0];
    assert_eq!(update["before"]["fields"]["1"]["Int64"], 100);
    assert_eq!(update["after"]["fields"]["1"]["Int64"], 101);
    assert_eq!(update["primary_key"], "10");

    let only_entity_2 = read_changes(db, 0, &[2]);
    assert_eq!(only_entity_2["events"].as_array().unwrap().len(), 2);

    let next: u64 = all["next_cursor"].as_str().unwrap().parse().unwrap();
    let mut end = 0;
    assert_eq!(adb_change_feed_end(db, &mut end), AdbStatus::Ok);
    assert_eq!(next, end);

    let name = b"indexer";
    let mut stored = 0;
    assert_eq!(
        adb_consumer_offset(db, name.as_ptr(), name.len(), &mut stored),
        AdbStatus::NotFound
    );
    assert_eq!(
        adb_commit_consumer_offset(db, name.as_ptr(), name.len(), next),
        AdbStatus::Ok
    );
    assert_eq!(
        adb_consumer_offset(db, name.as_ptr(), name.len(), &mut stored),
        AdbStatus::Ok
    );
    assert_eq!(stored, next);
    assert!(read_changes(db, next, &[])["events"]
        .as_array()
        .unwrap()
        .is_empty());

    let mut removed = 0;
    assert_eq!(adb_vacuum(db, &mut removed), AdbStatus::Ok);
    assert_eq!(removed, 1);
    assert_eq!(adb_checkpoint(db), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);

    // The feed and the offset survive the restart.
    let db = open(dir.path());
    assert_eq!(
        adb_consumer_offset(db, name.as_ptr(), name.len(), &mut stored),
        AdbStatus::Ok
    );
    assert_eq!(
        read_changes(db, 0, &[])["events"].as_array().unwrap().len(),
        4
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Zero `max_events`, a mid-frame cursor and a null handle are rejected and leave the output null.
#[test]
fn invalid_change_feed_arguments_are_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    insert(db, 1, 1, 1);
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_read_changes_json(db, 0, 0, ptr::null(), 0, &mut out),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_read_changes_json(db, 3, 10, ptr::null(), 0, &mut out),
        AdbStatus::InvalidArgument,
        "cursor inside a frame"
    );
    assert!(out.is_null());
    assert_eq!(
        adb_read_changes_json(ptr::null_mut(), 0, 10, ptr::null(), 0, &mut out),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// The JVM planner emits `entity_scan`; the native side must decode and execute it.
#[test]
fn entity_scan_plan_json_executes_over_the_c_abi() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for pk in 0..5 {
        insert(db, 7, pk, pk as i64);
        insert(db, 8, pk, pk as i64);
    }
    let plan = br#"{"wire_version":2,"plan":{"op":"entity_scan","entity_id":7,"columns":[{"field_id":1,"slot":0}]}}"#;
    let mut query = ptr::null_mut();
    assert_eq!(
        adb_execute_plan_json(db, plan.as_ptr(), plan.len(), &mut query),
        AdbStatus::Ok
    );
    let mut batch = ptr::null_mut();
    assert_eq!(adb_query_next_batch(query, &mut batch), AdbStatus::Ok);
    assert!(adb_batch_len(batch) > 0);
    assert_eq!(adb_batch_release(batch), AdbStatus::Ok);
    assert_eq!(
        adb_query_next_batch(query, &mut batch),
        AdbStatus::EndOfStream
    );
    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
