//! The projection registry over the C ABI (ABI 8).
use std::{ptr, thread, time::Duration};

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

/// Copies a JSON buffer out of its handle, releases it and parses it.
fn take_json(handle: *mut adb_ffi::handle::AdbBatchHandle) -> Value {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(handle), adb_batch_len(handle)) }
            .to_vec();
    assert_eq!(adb_batch_release(handle), AdbStatus::Ok);
    serde_json::from_slice(&bytes).unwrap()
}

/// Inserts `count` rows of entity 5.
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

/// Registers `name`.
fn register(db: Db, name: &str) -> u64 {
    let request = format!(r#"{{"name":"{name}","entity_id":5}}"#).into_bytes();
    let mut id = 0;
    assert_eq!(
        adb_projection_register_json(db, request.as_ptr(), request.len(), &mut id),
        AdbStatus::Ok
    );
    id
}

/// Register, list, get, advance, state changes and waiting.
#[test]
fn lifecycle() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    insert(db, 4);
    let id = register(db, "search");
    assert_eq!(
        register(db, "search"),
        id,
        "registration is idempotent by name"
    );

    let mut out = ptr::null_mut();
    assert_eq!(adb_projection_get_json(db, id, &mut out), AdbStatus::Ok);
    let doc = take_json(out);
    assert_eq!(doc["descriptor"]["name"], "search");
    assert_eq!(doc["state"], "Building");
    assert_eq!(doc["lag_commits"], 4);

    let mut out = ptr::null_mut();
    assert_eq!(adb_projection_list_json(db, &mut out), AdbStatus::Ok);
    let list = take_json(out);
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["descriptor"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["__current_row", "search"]);

    assert_eq!(adb_projection_advance(db, id, 2), AdbStatus::Ok);
    assert_eq!(
        adb_projection_advance(db, id, 1),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_projection_advance(db, id, 5),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_projection_set_state(db, id, 1), AdbStatus::Ok);

    // Already reached / times out / reached after another thread advances it.
    let mut outcome = 9;
    assert_eq!(
        adb_projection_wait_until(db, id, 2, 100, &mut outcome),
        AdbStatus::Ok
    );
    assert_eq!(outcome, 0);
    assert_eq!(
        adb_projection_wait_until(db, id, 4, 50, &mut outcome),
        AdbStatus::Ok
    );
    assert_eq!(outcome, 1);
    let address = db as usize;
    let advancer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let db = address as Db;
        assert_eq!(adb_projection_advance(db, id, 4), AdbStatus::Ok);
    });
    assert_eq!(
        adb_projection_wait_until(db, id, 4, 20_000, &mut outcome),
        AdbStatus::Ok
    );
    assert_eq!(outcome, 0);
    advancer.join().unwrap();

    assert_eq!(adb_projection_set_state(db, id, 3), AdbStatus::Ok);
    assert_eq!(
        adb_projection_wait_until(db, id, 9, 20_000, &mut outcome),
        AdbStatus::Ok
    );
    assert_eq!(outcome, 2);
    assert_eq!(
        adb_projection_wait_until(db, 424_242, 1, 10, &mut outcome),
        AdbStatus::Ok
    );
    assert_eq!(outcome, 3);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// The engine's own projections cannot be changed from outside; arguments are validated.
#[test]
fn validation() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    assert_eq!(adb_projection_advance(db, 1, 0), AdbStatus::InvalidArgument);
    assert_eq!(
        adb_projection_set_state(db, 1, 3),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_projection_advance(db, 999, 0), AdbStatus::NotFound);
    let id = register(db, "p");
    assert_eq!(
        adb_projection_set_state(db, id, 7),
        AdbStatus::InvalidArgument
    );
    let mut outcome = 5;
    assert_eq!(
        adb_projection_wait_until(db, id, 1, 60_001, &mut outcome),
        AdbStatus::InvalidArgument
    );
    assert_eq!(outcome, 3, "outputs are initialised first");
    let mut out: *mut adb_ffi::handle::AdbBatchHandle = ptr::dangling_mut();
    assert_eq!(
        adb_projection_get_json(db, 999, &mut out),
        AdbStatus::NotFound
    );
    assert!(out.is_null());
    for bad in [
        &br#"{"name":""}"#[..],
        br#"{"name":"x","extra":1}"#,
        b"nope",
    ] {
        let mut id = 7;
        assert_eq!(
            adb_projection_register_json(db, bad.as_ptr(), bad.len(), &mut id),
            AdbStatus::InvalidArgument
        );
        assert_eq!(id, 0);
    }
    assert_eq!(
        adb_projection_register_json(db, ptr::null(), 0, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
