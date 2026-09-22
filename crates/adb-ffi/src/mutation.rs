//! C ABI row mutation operations with bounded JSON inputs and deterministic output initialization.

use std::{collections::BTreeMap, slice};

use adb_core::{FieldId, Row, RowId, Value};

use crate::{
    error::{ffi_guard, map_db_error, MAX_MUTATION_JSON_BYTES},
    handle::AdbDatabaseHandle,
    AdbStatus,
};

/// Composes the Milestone 2 entity-scoped row identifier from entity and primary-key components.
fn compose_row_id(entity_id: u64, primary_key: u64) -> RowId {
    RowId(((entity_id as u128) << 64) | primary_key as u128)
}

/// Inserts one row encoded as bounded JSON and returns its commit timestamp.
#[no_mangle]
pub extern "C" fn adb_insert_row_json(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    primary_key: u64,
    row_ptr: *const u8,
    row_len: usize,
    out_commit_ts: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        prepare_json_call(db, row_ptr, row_len, out_commit_ts)?;
        let row_bytes = unsafe { slice::from_raw_parts(row_ptr, row_len) };
        let row: Row = serde_json::from_slice(row_bytes)
            .map_err(|error| (AdbStatus::InvalidArgument, error.to_string()))?;

        let database = &unsafe { &*db }.database;
        let row_id = compose_row_id(entity_id, primary_key);
        let mut tx = database.begin();
        if database
            .get_in_tx(&tx, row_id)
            .map_err(map_db_error)?
            .is_some()
        {
            return Err((AdbStatus::Conflict, "primary key already exists".into()));
        }
        tx.put(row_id, row);
        let commit_ts = database.commit(tx).map_err(map_db_error)?;
        unsafe { *out_commit_ts = commit_ts.0 };
        Ok(AdbStatus::Ok)
    })
}

/// Updates selected fields from a bounded JSON map and returns its commit timestamp.
#[no_mangle]
pub extern "C" fn adb_update_fields_json(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    primary_key: u64,
    assignments_ptr: *const u8,
    assignments_len: usize,
    out_commit_ts: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        prepare_json_call(db, assignments_ptr, assignments_len, out_commit_ts)?;
        let bytes = unsafe { slice::from_raw_parts(assignments_ptr, assignments_len) };
        let assignments: BTreeMap<FieldId, Value> = serde_json::from_slice(bytes)
            .map_err(|error| (AdbStatus::InvalidArgument, error.to_string()))?;

        let database = &unsafe { &*db }.database;
        let row_id = compose_row_id(entity_id, primary_key);
        let mut tx = database.begin();
        let Some(mut row) = database.get_in_tx(&tx, row_id).map_err(map_db_error)? else {
            return Err((AdbStatus::NotFound, "row not found".into()));
        };
        for (field_id, value) in assignments {
            row.fields.insert(field_id, value);
        }
        tx.put(row_id, row);
        let commit_ts = database.commit(tx).map_err(map_db_error)?;
        unsafe { *out_commit_ts = commit_ts.0 };
        Ok(AdbStatus::Ok)
    })
}

/// Deletes one entity-scoped row and returns the delete commit timestamp.
#[no_mangle]
pub extern "C" fn adb_delete_row(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    primary_key: u64,
    out_commit_ts: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        if out_commit_ts.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null commit output pointer".into(),
            ));
        }
        unsafe { *out_commit_ts = 0 };
        if db.is_null() {
            return Err((AdbStatus::InvalidArgument, "null database handle".into()));
        }
        let database = &unsafe { &*db }.database;
        let row_id = compose_row_id(entity_id, primary_key);
        let mut tx = database.begin();
        if database
            .get_in_tx(&tx, row_id)
            .map_err(map_db_error)?
            .is_none()
        {
            return Err((AdbStatus::NotFound, "row not found".into()));
        }
        tx.delete(row_id);
        let commit_ts = database.commit(tx).map_err(map_db_error)?;
        unsafe { *out_commit_ts = commit_ts.0 };
        Ok(AdbStatus::Ok)
    })
}

/// Validates common pointer/length contracts before constructing a Rust slice from native memory.
fn prepare_json_call(
    db: *mut AdbDatabaseHandle,
    input: *const u8,
    len: usize,
    out_commit_ts: *mut u64,
) -> Result<(), (AdbStatus, String)> {
    if out_commit_ts.is_null() {
        return Err((
            AdbStatus::InvalidArgument,
            "null commit output pointer".into(),
        ));
    }
    unsafe { *out_commit_ts = 0 };
    if db.is_null() || input.is_null() {
        return Err((AdbStatus::InvalidArgument, "null pointer".into()));
    }
    if len == 0 || len > MAX_MUTATION_JSON_BYTES {
        return Err((
            AdbStatus::InvalidArgument,
            format!("mutation JSON length must be 1..={MAX_MUTATION_JSON_BYTES}"),
        ));
    }
    Ok(())
}
