//! Query lifecycle over the C ABI: versioned plan input, snapshot selection, batch streaming and
//! runtime profiles.

use std::slice;

use adb_execution::encode_batch;
use parking_lot::Mutex;

use crate::{
    error::{ffi_guard, map_db_error, map_execution_error, MAX_PLAN_JSON_BYTES},
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
        let decoded = adb_plan_wire::decode_json(plan_bytes)
            .map_err(|error| (AdbStatus::InvalidArgument, error.to_string()))?;

        let database = &unsafe { &*db }.database;
        let cursor = database
            .execute_planned(decoded.plan, decoded.schema_epoch, snapshot)
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
                let bytes = encode_batch(&batch)
                    .map_err(|error| (AdbStatus::Internal, error.to_string()))?
                    .into_boxed_slice();
                unsafe { *out_batch = Box::into_raw(Box::new(AdbBatchHandle { bytes })) };
                Ok(AdbStatus::Ok)
            }
            Ok(None) => Ok(AdbStatus::EndOfStream),
            Err(error) => Err((map_execution_error(&error), error.to_string())),
        }
    })
}

/// Returns the query's per-operator runtime profile as a UTF-8 JSON buffer (read with
/// `adb_batch_data`/`adb_batch_len`, free with `adb_batch_release`). Complete once the query
/// reached its end; callable at any time.
#[no_mangle]
pub extern "C" fn adb_query_profile_json(
    query: *mut AdbQueryHandle,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        if out_json.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null profile output pointer".into(),
            ));
        }
        unsafe { *out_json = std::ptr::null_mut() };
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        let profile = unsafe { &*query }.cursor.lock().profile();
        let bytes = serde_json::to_vec(&profile)
            .map_err(|error| (AdbStatus::Internal, error.to_string()))?
            .into_boxed_slice();
        unsafe { *out_json = Box::into_raw(Box::new(AdbBatchHandle { bytes })) };
        Ok(AdbStatus::Ok)
    })
}

/// Counters of one query execution, written by [`adb_query_telemetry`] (ABI 9).
///
/// A plain C struct of four 64-bit integers (32 bytes, 8-byte alignment, no padding). The JVM
/// binding reads it from a reused native segment, so recording the telemetry of a query needs no
/// JSON document and no allocation. The field order is part of the ABI; new fields are only ever
/// added by a new ABI version.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdbQueryTelemetry {
    /// Rows returned to the caller.
    pub output_rows: u64,
    /// Rows read from storage: the rows produced by the plan's leaf operators.
    pub rows_read: u64,
    /// Batches returned to the caller.
    pub batches: u64,
    /// Highest number of bytes the query's blocking operators held at once.
    pub peak_memory_bytes: u64,
}

/// Writes the telemetry counters of `query` to `out`.
///
/// This is the cheap alternative to [`adb_query_profile_json`] for callers that only need totals:
/// it builds no profile tree and serializes nothing. The counters are complete once
/// `adb_query_next_batch` returned `ADB_END_OF_STREAM`; they can be read at any time before.
///
/// # Safety
///
/// `query` must be null or a live handle from `adb_execute_plan_json`, and `out` must be null or
/// point to writable, 8-byte-aligned memory of at least `size_of::<AdbQueryTelemetry>()` bytes. A
/// null pointer yields `InvalidArgument`; `out` is zeroed first so a failure never leaves stale
/// values behind.
#[no_mangle]
pub extern "C" fn adb_query_telemetry(
    query: *mut AdbQueryHandle,
    out: *mut AdbQueryTelemetry,
) -> AdbStatus {
    ffi_guard(|| {
        if out.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null telemetry output pointer".into(),
            ));
        }
        unsafe { *out = AdbQueryTelemetry::default() };
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        let counters = unsafe { &*query }.cursor.lock().telemetry();
        unsafe {
            *out = AdbQueryTelemetry {
                output_rows: counters.output_rows,
                rows_read: counters.source_rows,
                batches: counters.batches,
                peak_memory_bytes: counters.peak_memory_bytes,
            };
        }
        Ok(AdbStatus::Ok)
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
