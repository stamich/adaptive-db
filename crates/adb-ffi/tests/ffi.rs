//! Query lifecycle over the C ABI.
use std::ptr;

use adb_core::{RowId, Value};
use adb_engine::Database;
use adb_execution::PhysicalPlan;
use adb_ffi::{
    adb_batch_len, adb_batch_release, adb_close, adb_execute_plan_json, adb_open, adb_query_close,
    adb_query_next_batch, AdbStatus,
};
use tempfile::tempdir;

/// A point lookup plan runs through the full C ABI query lifecycle.
#[test]
fn ffi_executes_point_lookup() {
    let dir = tempdir().unwrap();

    {
        let db = Database::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.put(
            RowId(1),
            adb_core::Row::new().with_field(1, Value::Int64(123)),
        );
        db.commit(tx).unwrap();
    }

    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut db_handle = ptr::null_mut();
    assert_eq!(
        adb_open(path.as_ptr(), path.len(), &mut db_handle),
        AdbStatus::Ok
    );

    let plan = adb_plan_wire::encode_json(&PhysicalPlan::PointLookup {
        row_id: RowId(1),
        columns: Vec::new(),
    })
    .unwrap();

    let mut query = ptr::null_mut();
    assert_eq!(
        adb_execute_plan_json(db_handle, plan.as_ptr(), plan.len(), &mut query,),
        AdbStatus::Ok
    );

    let mut batch = ptr::null_mut();
    assert_eq!(adb_query_next_batch(query, &mut batch), AdbStatus::Ok);
    assert!(adb_batch_len(batch) > 0);

    assert_eq!(adb_batch_release(batch), AdbStatus::Ok);
    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db_handle), AdbStatus::Ok);
}
