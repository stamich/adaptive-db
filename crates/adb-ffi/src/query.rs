//! C ABI query lifecycle with bounded plan input and panic-contained execution.

use std::slice;

use adb_core::{FieldId, RowId};
use adb_execution::{encode_batch_v1, Expr, PhysicalPlan};
use parking_lot::Mutex;
use serde::Deserialize;

use crate::{
    error::{ffi_guard, map_db_error, MAX_PLAN_JSON_BYTES},
    handle::{AdbBatchHandle, AdbDatabaseHandle, AdbQueryHandle},
    AdbStatus,
};

/// FFI-only JSON representation of a physical plan.
///
/// `RowId` is a `u128` in the Rust engine. JSON numbers, however, are not a
/// portable 128-bit integer transport across JSON implementations. The C ABI
/// bootstrap protocol therefore accepts point-lookup row IDs either as an
/// unsigned 64-bit JSON number (the legacy/documented form) or as a decimal
/// string for the full `u128` range.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WirePhysicalPlan {
    PointLookup {
        row_id: WireRowId,
    },
    Scan,
    Filter {
        input: Box<WirePhysicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<WirePhysicalPlan>,
        fields: Vec<FieldId>,
    },
    Limit {
        input: Box<WirePhysicalPlan>,
        limit: usize,
    },
}

/// Stable JSON representation for `RowId` at the FFI boundary.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireRowId {
    Number(u64),
    Decimal(String),
}

impl WireRowId {
    fn into_row_id(self) -> Result<RowId, String> {
        match self {
            Self::Number(value) => Ok(RowId(u128::from(value))),
            Self::Decimal(value) => value
                .parse::<u128>()
                .map(RowId)
                .map_err(|_| "row_id decimal string is not a valid u128".to_string()),
        }
    }
}

impl WirePhysicalPlan {
    fn into_physical_plan(self) -> Result<PhysicalPlan, String> {
        Ok(match self {
            Self::PointLookup { row_id } => PhysicalPlan::PointLookup {
                row_id: row_id.into_row_id()?,
            },
            Self::Scan => PhysicalPlan::Scan,
            Self::Filter { input, predicate } => PhysicalPlan::Filter {
                input: Box::new(input.into_physical_plan()?),
                predicate,
            },
            Self::Project { input, fields } => PhysicalPlan::Project {
                input: Box::new(input.into_physical_plan()?),
                fields,
            },
            Self::Limit { input, limit } => PhysicalPlan::Limit {
                input: Box::new(input.into_physical_plan()?),
                limit,
            },
        })
    }
}

/// Decodes the bounded FFI JSON wire format into the Rust-native physical
/// plan. Keeping the wire DTO separate prevents JSON transport details from
/// leaking into engine-native ID serialization.
fn decode_plan_json(bytes: &[u8]) -> Result<PhysicalPlan, String> {
    let wire: WirePhysicalPlan =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    wire.into_physical_plan()
}

/// Decodes a bounded plan JSON and returns an owned query cursor handle.
#[no_mangle]
pub extern "C" fn adb_execute_plan_json(
    db: *mut AdbDatabaseHandle,
    plan_ptr: *const u8,
    plan_len: usize,
    out_query: *mut *mut AdbQueryHandle,
) -> AdbStatus {
    ffi_guard(|| {
        if out_query.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null query output pointer".into(),
            ));
        }
        unsafe {
            *out_query = std::ptr::null_mut();
        }
        if db.is_null() || plan_ptr.is_null() {
            return Err((AdbStatus::InvalidArgument, "null pointer".into()));
        }
        if plan_len == 0 || plan_len > MAX_PLAN_JSON_BYTES {
            return Err((
                AdbStatus::InvalidArgument,
                format!("plan length must be 1..={MAX_PLAN_JSON_BYTES}"),
            ));
        }

        let bytes = unsafe { slice::from_raw_parts(plan_ptr, plan_len) };
        let plan = decode_plan_json(bytes).map_err(|error| (AdbStatus::InvalidArgument, error))?;
        let cursor = unsafe { &*db }
            .database
            .execute(plan)
            .map_err(map_db_error)?;
        unsafe {
            *out_query = Box::into_raw(Box::new(AdbQueryHandle {
                cursor: Mutex::new(cursor),
            }));
        }
        Ok(AdbStatus::Ok)
    })
}

/// Produces the next encoded ADB Batch or `EndOfStream` and clears output on
/// all non-success paths.
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
        unsafe {
            *out_batch = std::ptr::null_mut();
        }
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        match unsafe { &*query }.cursor.lock().next_batch() {
            Ok(Some(batch)) => {
                let bytes = encode_batch_v1(&batch)
                    .map_err(|error| (AdbStatus::Internal, error.to_string()))?
                    .into_boxed_slice();
                unsafe {
                    *out_batch = Box::into_raw(Box::new(AdbBatchHandle { bytes }));
                }
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

/// Requests cooperative cancellation of a live query handle.
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

/// Releases one valid query handle exactly once.
#[no_mangle]
pub extern "C" fn adb_query_close(query: *mut AdbQueryHandle) -> AdbStatus {
    ffi_guard(|| {
        if query.is_null() {
            return Err((AdbStatus::InvalidArgument, "null query handle".into()));
        }
        unsafe {
            drop(Box::from_raw(query));
        }
        Ok(AdbStatus::Ok)
    })
}

#[cfg(test)]
mod tests {
    use super::decode_plan_json;
    use adb_core::RowId;
    use adb_execution::PhysicalPlan;

    #[test]
    fn decodes_numeric_point_lookup_row_id() {
        let plan = decode_plan_json(br#"{"op":"point_lookup","row_id":1}"#).unwrap();
        assert!(matches!(
            plan,
            PhysicalPlan::PointLookup { row_id: RowId(1) }
        ));
    }

    #[test]
    fn decodes_full_u128_point_lookup_row_id_as_decimal_string() {
        let expected = u128::MAX;
        let json = format!(r#"{{"op":"point_lookup","row_id":"{expected}"}}"#);
        let plan = decode_plan_json(json.as_bytes()).unwrap();
        assert!(matches!(
            plan,
            PhysicalPlan::PointLookup { row_id } if row_id == RowId(expected)
        ));
    }
}
