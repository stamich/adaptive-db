//! Single-row INSERT / UPDATE / DELETE, each in its own serializable transaction.

use std::collections::BTreeMap;

use adb_core::{FieldId, Row, RowId, Value};
use adb_engine::Database;

use crate::{
    args::{bytes, database, init_out, FfiError},
    error::{ffi_guard, map_db_error, MAX_MUTATION_JSON_BYTES},
    handle::AdbDatabaseHandle,
    AdbStatus,
};

/// Inserts a row (JSON `Row`); fails with `Conflict` if the primary key exists.
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
        init_out(out_commit_ts, 0)?;
        let database = database(db)?;
        let row: Row = parse(bytes(row_ptr, row_len, MAX_MUTATION_JSON_BYTES)?)?;
        let row_id = RowId::compose(entity_id, primary_key);
        commit_with(database, out_commit_ts, |tx| {
            if database
                .get_in_tx(tx, row_id)
                .map_err(map_db_error)?
                .is_some()
            {
                return Err((AdbStatus::Conflict, "primary key already exists".into()));
            }
            tx.put(row_id, row);
            Ok(())
        })
    })
}

/// Applies a JSON `{field_id: Value}` map to an existing row.
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
        init_out(out_commit_ts, 0)?;
        let database = database(db)?;
        let assignments: BTreeMap<FieldId, Value> = parse(bytes(
            assignments_ptr,
            assignments_len,
            MAX_MUTATION_JSON_BYTES,
        )?)?;
        let row_id = RowId::compose(entity_id, primary_key);
        commit_with(database, out_commit_ts, |tx| {
            let Some(mut row) = database.get_in_tx(tx, row_id).map_err(map_db_error)? else {
                return Err((AdbStatus::NotFound, "row not found".into()));
            };
            row.fields.extend(assignments);
            tx.put(row_id, row);
            Ok(())
        })
    })
}

/// Deletes an existing row.
#[no_mangle]
pub extern "C" fn adb_delete_row(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    primary_key: u64,
    out_commit_ts: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_commit_ts, 0)?;
        let database = database(db)?;
        let row_id = RowId::compose(entity_id, primary_key);
        commit_with(database, out_commit_ts, |tx| {
            if database
                .get_in_tx(tx, row_id)
                .map_err(map_db_error)?
                .is_none()
            {
                return Err((AdbStatus::NotFound, "row not found".into()));
            }
            tx.delete(row_id);
            Ok(())
        })
    })
}

/// Runs `body` in a fresh transaction and commits it, writing the commit timestamp.
fn commit_with(
    database: &Database,
    out_commit_ts: *mut u64,
    body: impl FnOnce(&mut adb_engine::Transaction) -> Result<(), FfiError>,
) -> Result<AdbStatus, FfiError> {
    let mut tx = database.begin();
    body(&mut tx)?;
    let commit_ts = database.commit(tx).map_err(map_db_error)?;
    init_out(out_commit_ts, commit_ts.0)?;
    Ok(AdbStatus::Ok)
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, FfiError> {
    serde_json::from_slice(bytes).map_err(|error| (AdbStatus::InvalidArgument, error.to_string()))
}
