//! Secondary indexes over the C ABI (ABI 6): create, list, drop, statuses and limits.
use std::ptr;

use adb_ffi::{
    adb_batch_data, adb_batch_len, adb_batch_release, adb_close, adb_create_index_json,
    adb_drop_index, adb_execute_plan_json, adb_insert_row_json, adb_last_error_len,
    adb_last_error_ptr, adb_list_indexes_json, adb_open, adb_query_close, adb_query_next_batch,
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};
use serde_json::Value;
use tempfile::{tempdir, TempDir};

/// Opens a fresh database over the ABI.
fn open() -> (TempDir, *mut AdbDatabaseHandle) {
    let dir = tempdir().unwrap();
    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut handle = ptr::null_mut();
    assert_eq!(
        adb_open(path.as_ptr(), path.len(), &mut handle),
        AdbStatus::Ok
    );
    (dir, handle)
}

/// Decodes and releases a JSON buffer.
fn take_json(buffer: *mut AdbBatchHandle) -> Value {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(buffer), adb_batch_len(buffer)) }
            .to_vec();
    assert_eq!(adb_batch_release(buffer), AdbStatus::Ok);
    serde_json::from_slice(&bytes).unwrap()
}

/// The current thread's last error message.
fn last_error() -> String {
    let bytes = unsafe { std::slice::from_raw_parts(adb_last_error_ptr(), adb_last_error_len()) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Inserts row `pk` of entity 3 with balance `balance` through the ABI.
fn insert(db: *mut AdbDatabaseHandle, pk: i64, balance: i64) {
    let row = format!(r#"{{"fields":{{"1":{{"Int64":{pk}}},"2":{{"Int64":{balance}}}}}}}"#);
    let mut ts = 0;
    assert_eq!(
        adb_insert_row_json(db, 3, pk as u64, row.as_ptr(), row.len(), &mut ts),
        AdbStatus::Ok
    );
}

/// Calls `adb_create_index_json` with `spec`.
fn create(db: *mut AdbDatabaseHandle, spec: &str) -> (AdbStatus, *mut AdbBatchHandle) {
    let mut out = ptr::null_mut();
    let status = adb_create_index_json(db, spec.as_ptr(), spec.len(), &mut out);
    (status, out)
}

/// Create returns the status, list shows it, drop removes it; queries can scan it.
#[test]
fn create_list_drop() {
    let (_dir, db) = open();
    for pk in -5..5 {
        insert(db, pk, pk * pk);
    }
    let (status, out) = create(
        db,
        r#"{"name":"idx_balance","entity_id":3,"columns":[{"field_id":2}],"include":[1]}"#,
    );
    assert_eq!(status, AdbStatus::Ok, "{}", last_error());
    let created = take_json(out);
    assert_eq!(created["name"], "idx_balance");
    assert_eq!(created["state"], "active");
    assert_eq!(created["columns"][0]["direction"], "asc");
    assert_eq!(created["usable_from_ts"], created["created_at_ts"]);

    let mut list = ptr::null_mut();
    assert_eq!(adb_list_indexes_json(db, &mut list), AdbStatus::Ok);
    let listed = take_json(list);
    assert_eq!(listed["indexes"].as_array().unwrap().len(), 1);
    assert_eq!(listed["indexes"][0]["id"], created["id"]);

    // A plan wire v3 index scan over the ABI: balance = 4 matches pk -2 and 2, in pk order.
    let plan = format!(
        r#"{{"wire_version":3,"plan":{{"op":"index_scan","index_id":{},"bounds":{{"prefix":[{{"Int64":4}}]}},"columns":[{{"field_id":1,"slot":0}}]}}}}"#,
        created["id"]
    );
    let mut query = ptr::null_mut();
    assert_eq!(
        adb_execute_plan_json(db, plan.as_ptr(), plan.len(), &mut query),
        AdbStatus::Ok,
        "{}",
        last_error()
    );
    let mut batch = ptr::null_mut();
    assert_eq!(adb_query_next_batch(query, &mut batch), AdbStatus::Ok);
    assert!(!batch.is_null());
    assert_eq!(adb_batch_release(batch), AdbStatus::Ok);
    assert_eq!(adb_query_close(query), AdbStatus::Ok);

    let name = "IDX_BALANCE";
    assert_eq!(adb_drop_index(db, name.as_ptr(), name.len()), AdbStatus::Ok);
    assert_eq!(
        adb_drop_index(db, name.as_ptr(), name.len()),
        AdbStatus::NotFound
    );
    let mut list = ptr::null_mut();
    assert_eq!(adb_list_indexes_json(db, &mut list), AdbStatus::Ok);
    assert!(take_json(list)["indexes"].as_array().unwrap().is_empty());
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Malformed and invalid specs, taken names and unindexable rows are invalid arguments; the
/// output slot stays null and the database keeps working.
#[test]
fn invalid_requests_are_rejected() {
    let (_dir, db) = open();
    for spec in [
        "not json",
        r#"{"name":"x","entity_id":3,"columns":[],"include":[]}"#,
        r#"{"name":"bad name","entity_id":3,"columns":[{"field_id":2}]}"#,
        r#"{"name":"x","entity_id":3,"columns":[{"field_id":2}],"unknown":1}"#,
        r#"{"name":"x","entity_id":3,"columns":[{"field_id":2,"direction":"sideways"}]}"#,
    ] {
        let (status, out) = create(db, spec);
        assert_eq!(status, AdbStatus::InvalidArgument, "{spec}");
        assert!(out.is_null());
    }
    let (status, out) = create(
        db,
        r#"{"name":"ok","entity_id":3,"columns":[{"field_id":2}]}"#,
    );
    assert_eq!(status, AdbStatus::Ok);
    take_json(out);
    let (status, _) = create(
        db,
        r#"{"name":"OK","entity_id":3,"columns":[{"field_id":2}]}"#,
    );
    assert_eq!(status, AdbStatus::InvalidArgument);
    assert!(last_error().contains("already exists"), "{}", last_error());

    let long = "x".repeat(600);
    let row = format!(r#"{{"fields":{{"2":{{"String":"{long}"}}}}}}"#);
    let mut ts = 0;
    assert_eq!(
        adb_insert_row_json(db, 3, 1, row.as_ptr(), row.len(), &mut ts),
        AdbStatus::InvalidArgument
    );
    assert!(last_error().contains("ok"), "{}", last_error());
    insert(db, 2, 2);

    let oversized = "y".repeat(70 * 1024);
    let (status, _) = create(db, &oversized);
    assert_eq!(status, AdbStatus::InvalidArgument);
    assert_eq!(
        adb_drop_index(db, ptr::null(), 1),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// A directory written before 2.3.0 is refused with its own status.
#[test]
fn older_data_formats_are_reported() {
    let dir = tempdir().unwrap();
    {
        let db = adb_engine::Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.put(adb_core::RowId::compose(1, 1), adb_core::Row::new());
        db.commit(tx).unwrap();
        db.close().unwrap();
    }
    std::fs::remove_file(dir.path().join(adb_engine::format::FORMAT_FILE)).unwrap();
    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut handle = ptr::null_mut();
    assert_eq!(
        adb_open(path.as_ptr(), path.len(), &mut handle),
        AdbStatus::IncompatibleFormat
    );
    assert!(handle.is_null());
    assert!(last_error().contains("adb-migrate"));
}
