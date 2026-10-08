//! Relational queries over the C ABI: plan wire v2, derived batches, profiles, status mapping.
use std::ptr;

use adb_core::{Row, RowId, Value};
use adb_engine::Database;
use adb_execution::{
    AggregateFunction, AggregateSpec, JoinKey, JoinType, PhysicalPlan, ScanColumn, SlotId,
    FLAG_ROW_IDS,
};
use adb_ffi::{
    adb_batch_data, adb_batch_len, adb_batch_release, adb_close, adb_execute_plan_json,
    adb_last_error_len, adb_last_error_ptr, adb_open, adb_query_close, adb_query_next_batch,
    adb_query_profile_json,
    handle::{AdbBatchHandle, AdbDatabaseHandle, AdbQueryHandle},
    AdbStatus,
};
use tempfile::{tempdir, TempDir};

/// Database with entity 1 (id, value) and entity 2 (id, owner -> entity 1); opened over the ABI.
fn database(values: &[i64]) -> (TempDir, *mut AdbDatabaseHandle) {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        for (n, value) in values.iter().enumerate() {
            let id = n as i64;
            tx.put(
                RowId::compose(1, n as u64),
                Row::new()
                    .with_field(1, Value::Int64(id))
                    .with_field(2, Value::Int64(*value)),
            );
            tx.put(
                RowId::compose(2, n as u64),
                Row::new()
                    .with_field(1, Value::Int64(100 + id))
                    .with_field(2, Value::Int64(id)),
            );
        }
        db.commit(tx).unwrap();
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

/// Scan columns from `(field, slot)` pairs.
fn columns(pairs: &[(u32, u32)]) -> Vec<ScanColumn> {
    pairs
        .iter()
        .map(|(field_id, slot)| ScanColumn {
            field_id: *field_id,
            slot: SlotId(*slot),
        })
        .collect()
}

/// `SELECT SUM(e1.value) FROM e2 JOIN e1 ON e2.owner = e1.id`.
fn join_sum() -> PhysicalPlan {
    PhysicalPlan::Aggregate {
        input: Box::new(PhysicalPlan::HashJoin {
            left: Box::new(PhysicalPlan::EntityScan {
                entity_id: 2,
                columns: columns(&[(2, 0)]),
            }),
            right: Box::new(PhysicalPlan::EntityScan {
                entity_id: 1,
                columns: columns(&[(1, 1), (2, 2)]),
            }),
            join_type: JoinType::Inner,
            keys: vec![JoinKey {
                left: SlotId(0),
                right: SlotId(1),
            }],
            residual: None,
        }),
        group_by: Vec::new(),
        aggregates: vec![AggregateSpec {
            function: AggregateFunction::Sum,
            input: Some(SlotId(2)),
            output: SlotId(3),
        }],
    }
}

/// Starts `plan_json` and returns the query handle (or the failing status).
fn execute(db: *mut AdbDatabaseHandle, plan_json: &[u8]) -> Result<*mut AdbQueryHandle, AdbStatus> {
    let mut query = ptr::null_mut();
    match adb_execute_plan_json(db, plan_json.as_ptr(), plan_json.len(), &mut query) {
        AdbStatus::Ok => Ok(query),
        status => Err(status),
    }
}

/// Bytes of a library-owned buffer; releases it.
fn take(buffer: *mut AdbBatchHandle) -> Vec<u8> {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(buffer), adb_batch_len(buffer)) }
            .to_vec();
    assert_eq!(adb_batch_release(buffer), AdbStatus::Ok);
    bytes
}

/// The calling thread's last error message.
fn last_error() -> String {
    let bytes = unsafe { std::slice::from_raw_parts(adb_last_error_ptr(), adb_last_error_len()) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// A join + aggregate plan runs over the ABI; its batch carries no row ids, its single INT64
/// column is the output slot, and the profile describes the operator tree.
#[test]
fn join_aggregate_runs_and_reports_a_profile() {
    let (_dir, db) = database(&[1, 2, 3, 4]);
    let query = execute(db, &adb_plan_wire::encode_json(&join_sum()).unwrap()).unwrap();

    let mut batch = ptr::null_mut();
    assert_eq!(adb_query_next_batch(query, &mut batch), AdbStatus::Ok);
    let bytes = take(batch);
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    assert_eq!(flags & FLAG_ROW_IDS, 0, "aggregate rows have no row ids");
    let rows = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    assert_eq!(rows, 1);
    // header (16) + column header: slot id 3, type INT64 (2)
    assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 3);
    assert_eq!(bytes[20], 2);
    let sum = i64::from_le_bytes(bytes[bytes.len() - 8..].try_into().unwrap());
    assert_eq!(sum, 10);
    assert_eq!(
        adb_query_next_batch(query, &mut batch),
        AdbStatus::EndOfStream
    );

    let mut json = ptr::null_mut();
    assert_eq!(adb_query_profile_json(query, &mut json), AdbStatus::Ok);
    let profile: serde_json::Value = serde_json::from_slice(&take(json)).unwrap();
    assert_eq!(profile["root"]["operator"], "aggregate");
    assert_eq!(profile["root"]["children"][0]["operator"], "hash_join");
    assert_eq!(profile["root"]["children"][0]["counters"]["build_rows"], 4);
    // ABI 5: pre-order node ids (aggregate 0, hash_join 1, its inputs 2 and 3).
    assert_eq!(profile["root"]["node_id"], 0);
    assert_eq!(profile["root"]["children"][0]["node_id"], 1);
    assert_eq!(profile["root"]["children"][0]["children"][0]["node_id"], 2);
    assert_eq!(profile["root"]["children"][0]["children"][1]["node_id"], 3);

    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// An INT64 SUM overflow surfaces as `ARITHMETIC_OVERFLOW`, not as a wrapped value.
#[test]
fn sum_overflow_maps_to_its_own_status() {
    let (_dir, db) = database(&[i64::MAX, 1]);
    let query = execute(db, &adb_plan_wire::encode_json(&join_sum()).unwrap()).unwrap();
    let mut batch = ptr::null_mut();
    assert_eq!(
        adb_query_next_batch(query, &mut batch),
        AdbStatus::ArithmeticOverflow
    );
    assert!(batch.is_null());
    assert!(last_error().contains("INT64"));
    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// A 2.0-style bare plan is refused with a message that names the version mismatch.
#[test]
fn bare_v1_plan_is_rejected_with_a_version_message() {
    let (_dir, db) = database(&[1]);
    assert_eq!(
        execute(db, br#"{"op":"entity_scan","entity_id":1}"#).unwrap_err(),
        AdbStatus::InvalidArgument
    );
    assert!(last_error().contains("wire version"));

    let invalid = br#"{"wire_version":2,"plan":{"op":"project","input":{"op":"scan","columns":[]},"slots":[1]}}"#;
    assert_eq!(
        execute(db, invalid).unwrap_err(),
        AdbStatus::InvalidArgument
    );
    assert!(last_error().contains("invalid plan"));
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
