//! Query telemetry counters over the C ABI (ABI 9): `adb_query_telemetry` reports the totals of an
//! execution without a JSON profile, agrees with the profile, and rejects bad arguments.
use std::{mem, ptr};

use adb_core::{Row, RowId, Value};
use adb_engine::Database;
use adb_execution::{
    AggregateFunction, AggregateSpec, JoinKey, JoinType, PhysicalPlan, ScanColumn, SlotId,
};
use adb_ffi::{
    adb_batch_data, adb_batch_len, adb_batch_release, adb_close, adb_execute_plan_json, adb_open,
    adb_query_close, adb_query_next_batch, adb_query_profile_json, adb_query_telemetry,
    handle::{AdbBatchHandle, AdbDatabaseHandle, AdbQueryHandle},
    AdbQueryTelemetry, AdbStatus,
};
use tempfile::{tempdir, TempDir};

/// Opens a database with entity 1 (id, value) and entity 2 (id, owner -> entity 1), `rows` rows each.
fn database(rows: u64) -> (TempDir, *mut AdbDatabaseHandle) {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        for n in 0..rows {
            let id = n as i64;
            tx.put(
                RowId::compose(1, n),
                Row::new()
                    .with_field(1, Value::Int64(id))
                    .with_field(2, Value::Int64(id * 10)),
            );
            tx.put(
                RowId::compose(2, n),
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

/// `SELECT SUM(e1.value) FROM e2 JOIN e1 ON e2.owner = e1.id`: two scans under a join.
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

/// Starts the join plan and returns its query handle.
fn start(db: *mut AdbDatabaseHandle) -> *mut AdbQueryHandle {
    let json = adb_plan_wire::encode_json(&join_sum(), adb_core::SchemaEpoch(0)).unwrap();
    let mut query = ptr::null_mut();
    assert_eq!(
        adb_execute_plan_json(db, json.as_ptr(), json.len(), &mut query),
        AdbStatus::Ok
    );
    query
}

/// Bytes of a library-owned buffer; releases it.
fn take(buffer: *mut AdbBatchHandle) -> Vec<u8> {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(buffer), adb_batch_len(buffer)) }
            .to_vec();
    assert_eq!(adb_batch_release(buffer), AdbStatus::Ok);
    bytes
}

/// The C struct is four 64-bit integers: 32 bytes, 8-byte aligned, no padding.
#[test]
fn layout_is_four_u64() {
    assert_eq!(mem::size_of::<AdbQueryTelemetry>(), 32);
    assert_eq!(mem::align_of::<AdbQueryTelemetry>(), 8);
}

/// After the stream ended, the counters equal what the JSON profile reports.
#[test]
fn counters_agree_with_the_profile() {
    let (_dir, db) = database(4);
    let query = start(db);
    let mut batch = ptr::null_mut();
    assert_eq!(adb_query_next_batch(query, &mut batch), AdbStatus::Ok);
    take(batch);
    assert_eq!(
        adb_query_next_batch(query, &mut batch),
        AdbStatus::EndOfStream
    );

    let mut counters = AdbQueryTelemetry::default();
    assert_eq!(adb_query_telemetry(query, &mut counters), AdbStatus::Ok);
    assert_eq!(counters.output_rows, 1, "one aggregate row");
    assert_eq!(counters.batches, 1);
    assert_eq!(counters.rows_read, 8, "4 rows from each scan");

    let mut json = ptr::null_mut();
    assert_eq!(adb_query_profile_json(query, &mut json), AdbStatus::Ok);
    let profile: serde_json::Value = serde_json::from_slice(&take(json)).unwrap();
    let inputs = &profile["root"]["children"][0]["children"];
    let leaves: u64 = inputs
        .as_array()
        .unwrap()
        .iter()
        .map(|leaf| leaf["rows_out"].as_u64().unwrap())
        .sum();
    assert_eq!(counters.rows_read, leaves);
    assert_eq!(counters.output_rows, profile["root"]["rows_out"]);
    assert_eq!(
        counters.peak_memory_bytes,
        profile["peak_memory_bytes"].as_u64().unwrap()
    );

    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Reading the counters before the first batch is allowed and reports nothing returned yet.
#[test]
fn counters_can_be_read_before_the_first_batch() {
    let (_dir, db) = database(3);
    let query = start(db);
    let mut counters = AdbQueryTelemetry {
        output_rows: 99,
        ..AdbQueryTelemetry::default()
    };
    assert_eq!(adb_query_telemetry(query, &mut counters), AdbStatus::Ok);
    assert_eq!(counters.output_rows, 0);
    assert_eq!(counters.batches, 0);
    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Null arguments are refused, and the output is zeroed before the query is looked at.
#[test]
fn null_arguments_are_rejected_and_the_output_is_zeroed() {
    let (_dir, db) = database(1);
    let query = start(db);
    let mut counters = AdbQueryTelemetry {
        output_rows: 7,
        rows_read: 7,
        batches: 7,
        peak_memory_bytes: 7,
    };
    assert_eq!(
        adb_query_telemetry(ptr::null_mut(), &mut counters),
        AdbStatus::InvalidArgument
    );
    assert_eq!(counters, AdbQueryTelemetry::default(), "no stale values");
    assert_eq!(
        adb_query_telemetry(query, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
