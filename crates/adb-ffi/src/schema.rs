//! Entity schemas over the C ABI (ABI 7, Milestone 2.4.0): create, alter, drop, list and
//! version history.
//!
//! Definitions travel as JSON in the forms of `adb-schema` (see docs/schema.md). A create
//! request is an `EntitySpec`:
//!
//! ```json
//! {"name":"account","entity_id":3,"primary_key":"id","fields":[
//!   {"name":"id","value_type":"Int64","nullable":false},
//!   {"name":"balance","value_type":"Int64","nullable":false,"default":{"Int64":0}}]}
//! ```
//!
//! an alter request names the entity and one `SchemaChange`:
//!
//! ```json
//! {"entity_id":3,"change":{"op":"rename_field","from":"balance","to":"amount"}}
//! ```
//!
//! and every schema returned (by create, alter, drop, list and history) is an `EntitySchema`
//! with its version, parent version, catalog epoch and every field ever defined (dropped
//! fields carry `dropped_in`). Each successful DDL call commits one catalog record to the log
//! and advances the catalog epoch, so plans made before it are refused with
//! `ADB_SCHEMA_CHANGED`.
use adb_engine::{EntitySchema, EntitySpec, SchemaChange};
use serde::Deserialize;
use serde_json::json;

use crate::{
    args::{bytes, database, init_out, FfiError},
    error::{ffi_guard, map_db_error},
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};

/// Largest schema definition or change request accepted, in bytes.
pub const MAX_SCHEMA_JSON_BYTES: usize = 1024 * 1024;

/// Body of an `adb_alter_entity_json` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AlterRequest {
    /// Entity to change.
    entity_id: u64,
    /// The change, applied to the entity's latest schema.
    change: SchemaChange,
}

/// Creates an entity from a JSON `EntitySpec` and writes its first schema (version 1) as a JSON
/// buffer (read with `adb_batch_data`/`adb_batch_len`, free with `adb_batch_release`).
///
/// Errors: `ADB_INVALID_ARGUMENT` for a malformed or invalid definition or a taken name or id;
/// `ADB_RESOURCE_LIMIT` when a schema limit (fields, entities, name or default size) is exceeded.
#[no_mangle]
pub extern "C" fn adb_create_entity_json(
    db: *mut AdbDatabaseHandle,
    spec_ptr: *const u8,
    spec_len: usize,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let database = database(db)?;
        let spec: EntitySpec = parse(spec_ptr, spec_len, "entity spec")?;
        let schema = database.create_entity(spec).map_err(map_db_error)?;
        write_json(&schema_json(&schema)?, out_json)
    })
}

/// Applies one schema change to a live entity and writes the new schema version as a JSON
/// buffer.
///
/// Errors: `ADB_NOT_FOUND` for an unknown or dropped entity, `ADB_INVALID_ARGUMENT` for a
/// malformed or refused change (unknown field, taken name, dropping the primary key or an
/// indexed field, a `NOT NULL` field without a default, ...).
#[no_mangle]
pub extern "C" fn adb_alter_entity_json(
    db: *mut AdbDatabaseHandle,
    request_ptr: *const u8,
    request_len: usize,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let database = database(db)?;
        let request: AlterRequest = parse(request_ptr, request_len, "alter request")?;
        let schema = database
            .alter_entity(request.entity_id, &request.change)
            .map_err(map_db_error)?;
        write_json(&schema_json(&schema)?, out_json)
    })
}

/// Drops the entity `entity_id`. Its rows stay in the log (history and `AS OF` reads of older
/// snapshots are unaffected) but the name can be reused.
///
/// Errors: `ADB_NOT_FOUND` for an unknown or already dropped entity, `ADB_INVALID_ARGUMENT`
/// while indexes of the entity exist (drop them first).
#[no_mangle]
pub extern "C" fn adb_drop_entity(db: *mut AdbDatabaseHandle, entity_id: u64) -> AdbStatus {
    ffi_guard(|| {
        database(db)?.drop_entity(entity_id).map_err(map_db_error)?;
        Ok(AdbStatus::Ok)
    })
}

/// Writes the catalog as `{"schema_epoch":N,"entities":[schema, ...]}`: the latest schema of
/// every live entity, in entity id order, and the epoch they belong to (one consistent view).
#[no_mangle]
pub extern "C" fn adb_list_schemas_json(
    db: *mut AdbDatabaseHandle,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let (epoch, schemas) = database(db)?.catalog_snapshot();
        let entities = schemas
            .iter()
            .map(|schema| schema_json(schema))
            .collect::<Result<Vec<_>, _>>()?;
        write_json(
            &json!({ "schema_epoch": epoch.0, "entities": entities }),
            out_json,
        )
    })
}

/// Writes every schema version of `entity_id` (also of a dropped entity) as
/// `{"entity_id":N,"versions":[schema, ...]}`, oldest first.
///
/// Errors: `ADB_NOT_FOUND` if the entity never had a schema.
#[no_mangle]
pub extern "C" fn adb_schema_history_json(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let versions = database(db)?.schema_history(entity_id);
        if versions.is_empty() {
            return Err((
                AdbStatus::NotFound,
                format!("entity {entity_id} has no schema"),
            ));
        }
        let versions = versions
            .iter()
            .map(|schema| schema_json(schema))
            .collect::<Result<Vec<_>, _>>()?;
        write_json(
            &json!({ "entity_id": entity_id, "versions": versions }),
            out_json,
        )
    })
}

/// Reads and decodes a bounded JSON request; `what` names it in the error message.
fn parse<T: serde::de::DeserializeOwned>(
    ptr: *const u8,
    len: usize,
    what: &str,
) -> Result<T, FfiError> {
    serde_json::from_slice(bytes(ptr, len, MAX_SCHEMA_JSON_BYTES)?).map_err(|error| {
        (
            AdbStatus::InvalidArgument,
            format!("invalid {what}: {error}"),
        )
    })
}

/// The JSON form of one schema version.
fn schema_json(schema: &EntitySchema) -> Result<serde_json::Value, FfiError> {
    serde_json::to_value(schema).map_err(|error| (AdbStatus::Internal, error.to_string()))
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
