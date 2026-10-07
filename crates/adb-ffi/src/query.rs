//! Query lifecycle over the C ABI: bounded plan input, snapshot selection, batch streaming.

use std::slice;

use adb_execution::{encode_batch_v1, PhysicalPlan};
use parking_lot::Mutex;

use crate::{
    error::{ffi_guard, map_db_error, MAX_PLAN_JSON_BYTES},
    handle::{AdbBatchHandle, AdbDatabaseHandle, AdbQueryHandle},
    AdbStatus,
};

/// Parses and executes a bounded physical-plan JSON document at the latest committed snapshot.
#[no_mangle]
pub extern "C" fn adb_execute_plan_json(
    db: *mut AdbDatabaseHandle,
    plan_ptr: *const u8,
    plan_len: usize,
    out_query: *mut *mut AdbQueryHandle,
) -> AdbStatus {
    execute_plan(db, plan_ptr, plan_len, None, out_query)
}

/// Parses and executes a bounded physical-plan JSON document at an explicit historical snapshot.
#[no_mangle]
pub extern "C" fn adb_execute_plan_json_at(
    db: *mut AdbDatabaseHandle,
    plan_ptr: *const u8,
    plan_len: usize,
    snapshot_ts: u64,
    out_query: *mut *mut AdbQueryHandle,
) -> AdbStatus {
    execute_plan(
        db,
        plan_ptr,
        plan_len,
        Some(adb_core::CommitTs(snapshot_ts)),
        out_query,
    )
}

/// Shared implementation for latest and historical query creation.
fn execute_plan(
    db: *mut AdbDatabaseHandle,
    plan_ptr: *const u8,
    plan_len: usize,
    snapshot: Option<adb_core::CommitTs>,
    out_query: *mut *mut AdbQueryHandle,
) -> AdbStatus {
    ffi_guard(|| {
        if out_query.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null query output pointer".into(),
            ));
        }
        unsafe { *out_query = std::ptr::null_mut() };
        if db.is_null() || plan_ptr.is_null() {
            return Err((AdbStatus::InvalidArgument, "null pointer".into()));
        }
        if plan_len == 0 || plan_len > MAX_PLAN_JSON_BYTES {
            return Err((
                AdbStatus::InvalidArgument,
                format!("plan length must be 1..={MAX_PLAN_JSON_BYTES}"),
            ));
        }

        let plan_bytes = unsafe { slice::from_raw_parts(plan_ptr, plan_len) };
        let plan: PhysicalPlan = serde_json::from_slice(plan_bytes)
            .map_err(|error| (AdbStatus::InvalidArgument, error.to_string()))?;

        let database = &unsafe { &*db }.database;
        let cursor = match snapshot {
            Some(ts) => database.execute_at(plan, ts),
            None => database.execute(plan),
        }
        .map_err(map_db_error)?;

        unsafe {
            *out_query = Box::into_raw(Box::new(AdbQueryHandle {
                cursor: Mutex::new(cursor),
            }))
        };
        Ok(AdbStatus::Ok)
    })
}

/// Produces the next encoded ADB Batch, clearing the output handle on all non-success paths.
#[no_mangle]
pub extern "C" fn adb_query_next_batch(
    query: *mut AdbQueryHandle,
    out_batch: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        if out_batch.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null batch output pointer".into(),
            ));
        }
        unsafe { *out_batch = std::ptr::null_mut() };
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        match unsafe { &*query }.cursor.lock().next_batch() {
            Ok(Some(batch)) => {
                let bytes = encode_batch_v1(&batch)
                    .map_err(|error| (AdbStatus::Internal, error.to_string()))?
                    .into_boxed_slice();
                unsafe { *out_batch = Box::into_raw(Box::new(AdbBatchHandle { bytes })) };
                Ok(AdbStatus::Ok)
            }
            Ok(None) => Ok(AdbStatus::EndOfStream),
            Err(adb_execution::ExecutionError::Cancelled) => {
                Err((AdbStatus::Cancelled, "query cancelled".into()))
            }
            Err(error) => Err((AdbStatus::Internal, error.to_string())),
        }
    })
}

/// Requests cooperative cancellation of a valid live query handle.
#[no_mangle]
pub extern "C" fn adb_query_cancel(query: *mut AdbQueryHandle) -> AdbStatus {
    ffi_guard(|| {
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        unsafe { &*query }.cursor.lock().cancel();
        Ok(AdbStatus::Ok)
    })
}

/// Releases a valid query handle exactly once.
#[no_mangle]
pub extern "C" fn adb_query_close(query: *mut AdbQueryHandle) -> AdbStatus {
    ffi_guard(|| {
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        unsafe { drop(Box::from_raw(query)) };
        Ok(AdbStatus::Ok)
    })
}
