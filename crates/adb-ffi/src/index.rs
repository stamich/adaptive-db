//! Secondary indexes over the C ABI (ABI 6): create, drop and list.
//!
//! Definitions travel as JSON. A create request is an `IndexSpec`:
//!
//! ```json
//! {"name":"idx_balance","entity_id":3,"columns":[{"field_id":2,"direction":"asc"}],"include":[3]}
//! ```
//!
//! and every listed index (also the result of a create) is an index status:
//!
//! ```json
//! {"id":0,"name":"idx_balance","entity_id":3,"columns":[{"field_id":2,"direction":"asc"}],
//!  "include":[3],"state":"active","created_at_ts":120,"usable_from_ts":120,"watermark_ts":131}
//! ```
//!
//! `usable_from_ts..=watermark_ts` is the range of snapshots the index serves; a planner uses
//! the index only for snapshots in that range. Index statistics are part of the entity's
//! statistics document (`adb_statistics_json`, format 2).
use adb_engine::{IndexSpec, IndexStatus};
use serde_json::json;

use crate::{
    args::{bytes, database, init_out, utf8, FfiError},
    error::{ffi_guard, map_db_error},
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};

/// Largest index specification accepted, in bytes.
pub const MAX_INDEX_SPEC_BYTES: usize = 64 * 1024;
/// Longest index name accepted by `adb_drop_index`, in bytes.
pub const MAX_INDEX_NAME_BYTES: usize = adb_engine::index_catalog::MAX_INDEX_NAME_BYTES;

/// Creates and builds an index from a JSON `IndexSpec`; writes its status as a JSON buffer
/// (read with `adb_batch_data`/`adb_batch_len`, free with `adb_batch_release`).
///
/// Errors: `ADB_INVALID_ARGUMENT` for a malformed or invalid spec, a taken name or a row of the
/// entity that cannot be indexed (key too large).
#[no_mangle]
pub extern "C" fn adb_create_index_json(
    db: *mut AdbDatabaseHandle,
    spec_ptr: *const u8,
    spec_len: usize,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let database = database(db)?;
        let spec: IndexSpec =
            serde_json::from_slice(bytes(spec_ptr, spec_len, MAX_INDEX_SPEC_BYTES)?).map_err(
                |error| {
                    (
                        AdbStatus::InvalidArgument,
                        format!("invalid index spec: {error}"),
                    )
                },
            )?;
        let created = database.create_index(spec).map_err(map_db_error)?;
        let status = database
            .indexes()
            .into_iter()
            .find(|status| status.definition.id == created.id)
            .ok_or_else(|| {
                (
                    AdbStatus::Internal,
                    "created index is not listed".to_string(),
                )
            })?;
        write_json(&status_json(&status), out_json)
    })
}

/// Drops the index named by the UTF-8 string `name`; `ADB_NOT_FOUND` if there is none.
#[no_mangle]
pub extern "C" fn adb_drop_index(
    db: *mut AdbDatabaseHandle,
    name_ptr: *const u8,
    name_len: usize,
) -> AdbStatus {
    ffi_guard(|| {
        let database = database(db)?;
        let name = utf8(name_ptr, name_len, MAX_INDEX_NAME_BYTES)?;
        database.drop_index(name).map_err(map_db_error)?;
        Ok(AdbStatus::Ok)
    })
}

/// Writes every index as `{"indexes":[status, ...]}` (id order) into a JSON buffer.
#[no_mangle]
pub extern "C" fn adb_list_indexes_json(
    db: *mut AdbDatabaseHandle,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let statuses: Vec<serde_json::Value> =
            database(db)?.indexes().iter().map(status_json).collect();
        write_json(&json!({ "indexes": statuses }), out_json)
    })
}

/// The JSON form of one index status (see the module documentation).
fn status_json(status: &IndexStatus) -> serde_json::Value {
    let definition = &status.definition;
    json!({
        "id": definition.id.0,
        "name": definition.name,
        "entity_id": definition.entity_id,
        "columns": definition.columns,
        "include": definition.include,
        "state": definition.state,
        "created_at_ts": definition.created_at_ts.0,
        "usable_from_ts": status.usable_from.0,
        "watermark_ts": status.watermark.0,
    })
}

/// Serializes `value` into a library-owned buffer stored in `out_json`.
fn write_json(
    value: &serde_json::Value,
    out_json: *mut *mut AdbBatchHandle,
) -> Result<AdbStatus, FfiError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| (AdbStatus::Internal, error.to_string()))?
        .into_boxed_slice();
    init_out(out_json, Box::into_raw(Box::new(AdbBatchHandle { bytes })))?;
    Ok(AdbStatus::Ok)
}
