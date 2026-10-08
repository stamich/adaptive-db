//! Optimizer statistics over the C ABI (ABI 5): ANALYZE, documents, staleness, statuses.
use std::ptr;

use adb_core::{Row, RowId, Value};
use adb_engine::{Database, TableStatistics};
use adb_ffi::{
    adb_analyze_entity_json, adb_batch_data, adb_batch_len, adb_batch_release, adb_close,
    adb_last_error_len, adb_last_error_ptr, adb_modifications_since_analyze, adb_open,
    adb_statistics_generation, adb_statistics_json,
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};
use tempfile::{tempdir, TempDir};

/// Entity of the test rows.
const ENTITY: u64 = 4;

/// Database with 30 rows of [`ENTITY`] (field 1 = pk, field 2 = pk % 3), opened over the ABI.
fn database() -> (TempDir, *mut AdbDatabaseHandle) {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        for pk in 0..30u64 {
            let mut tx = db.begin();
            tx.put(
                RowId::compose(ENTITY, pk),
                Row::new()
                    .with_field(1, Value::Int64(pk as i64))
                    .with_field(2, Value::Int64((pk % 3) as i64)),
            );
            db.commit(tx).unwrap();
        }
        db.close().unwrap();
    }
    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut handle = ptr::null_mut();
    assert_eq!(
        adb_open(path.as_ptr(), path.len(), &mut handle),
        AdbStatus::Ok
    );
    (dir, handle)
}

/// Decodes and releases a JSON buffer.
fn take_document(buffer: *mut AdbBatchHandle) -> TableStatistics {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(buffer), adb_batch_len(buffer)) }
            .to_vec();
    assert_eq!(adb_batch_release(buffer), AdbStatus::Ok);
    TableStatistics::from_json(&bytes).unwrap()
}

/// The current thread's last error message.
fn last_error() -> String {
    let bytes = unsafe { std::slice::from_raw_parts(adb_last_error_ptr(), adb_last_error_len()) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// ANALYZE returns the document, the read call returns the same one, and the counter resets.
#[test]
fn analyze_and_read_statistics() {
    let (_dir, db) = database();
    let mut count = u64::MAX;
    assert_eq!(
        adb_modifications_since_analyze(db, ENTITY, &mut count),
        AdbStatus::Ok
    );
    assert_eq!(count, 30);

    let mut buffer = ptr::null_mut();
    assert_eq!(
        adb_statistics_json(db, ENTITY, &mut buffer),
        AdbStatus::NotFound
    );
    assert!(buffer.is_null());
    assert!(last_error().contains("ANALYZE"));

    let options = br#"{"histogram_buckets":4}"#;
    assert_eq!(
        adb_analyze_entity_json(db, ENTITY, options.as_ptr(), options.len(), &mut buffer),
        AdbStatus::Ok
    );
    let analyzed = take_document(buffer);
    assert_eq!(analyzed.row_count, 30);
    assert_eq!(analyzed.column(2).unwrap().distinct_count, 3);
    assert_eq!(analyzed.column(1).unwrap().histogram.len(), 4);
    assert_eq!(analyzed.modifications_at_analyze, 30);

    let mut buffer = ptr::null_mut();
    assert_eq!(adb_statistics_json(db, ENTITY, &mut buffer), AdbStatus::Ok);
    assert_eq!(take_document(buffer), analyzed);
    assert_eq!(
        adb_modifications_since_analyze(db, ENTITY, &mut count),
        AdbStatus::Ok
    );
    assert_eq!(count, 0);
    let mut generation = 0;
    assert_eq!(
        adb_statistics_generation(db, ENTITY, &mut generation),
        AdbStatus::Ok
    );
    assert!(generation > 0);
    assert_eq!(
        adb_statistics_generation(db, ENTITY, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );

    // Default options: zero-length input with a null pointer.
    let mut buffer = ptr::null_mut();
    assert_eq!(
        adb_analyze_entity_json(db, ENTITY, ptr::null(), 0, &mut buffer),
        AdbStatus::Ok
    );
    assert_eq!(take_document(buffer).column(1).unwrap().histogram.len(), 30);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Bad options, limits and bad pointers map onto stable statuses with null outputs.
#[test]
fn statistics_errors_map_to_statuses() {
    let (_dir, db) = database();
    let mut buffer = ptr::null_mut();
    for (options, status) in [
        (&br#"{"sample_rows":0}"#[..], AdbStatus::InvalidArgument),
        (&br#"{"unknown":1}"#[..], AdbStatus::InvalidArgument),
        (&b"not json"[..], AdbStatus::InvalidArgument),
        (&br#"{"max_rows":10}"#[..], AdbStatus::ResourceLimit),
    ] {
        assert_eq!(
            adb_analyze_entity_json(db, ENTITY, options.as_ptr(), options.len(), &mut buffer),
            status,
            "{}",
            String::from_utf8_lossy(options)
        );
        assert!(buffer.is_null());
    }
    assert_eq!(
        adb_analyze_entity_json(ptr::null_mut(), ENTITY, ptr::null(), 0, &mut buffer),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_analyze_entity_json(db, ENTITY, ptr::null(), 5, &mut buffer),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_statistics_json(db, ENTITY, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_modifications_since_analyze(db, ENTITY, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
