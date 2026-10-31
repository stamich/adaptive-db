//! Change-data capture over the C ABI.
//!
//! Changes are returned as one UTF-8 JSON document in an `AdbBatchHandle` (read it with
//! `adb_batch_data`/`adb_batch_len`, free it with `adb_batch_release`):
//!
//! ```json
//! {"next_cursor":"8589934720",
//!  "events":[{"tx_id":5,"commit_ts":5,"commit_lsn":"420","cursor_after":"512",
//!             "changes":[{"entity_id":1,"primary_key":"7","kind":"update",
//!                         "schema_version":2,
//!                         "before":{"fields":{"1":{"Int64":10}},"schema_version":1},
//!                         "after":{"fields":{"1":{"Int64":20}},"schema_version":2}}]}]}
//! ```
//!
//! Log positions (`u64`) and primary keys (`i64`, the SQL value) are decimal strings.
//!
//! Since ABI 7 every row change carries `schema_version` (0 for rows written without a schema;
//! rows are delivered as stored), and DDL appears in `changes` too, before the row changes of
//! its transaction: `{"entity_id":1,"kind":"schema","epoch":3,"schema":{...}}` for a new schema
//! version (the document of `adb_schema_history_json`) and
//! `{"entity_id":1,"kind":"drop_table","epoch":4}` for a dropped table. Consumers dispatch on
//! `kind`; index DDL is not part of the feed.

use std::slice;

use adb_core::Lsn;
use adb_engine::{CatalogChange, ChangeBatch, ChangeCursor, ChangeEvent, ChangeFilter, ChangeKind};
use serde_json::{json, Value};

use crate::{
    args::{database, init_out, invalid, utf8},
    error::{ffi_guard, map_db_error, MAX_CHANGES_JSON_BYTES, MAX_CONSUMER_NAME_BYTES},
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};

/// Largest page size accepted from callers.
const MAX_EVENTS_PER_CALL: u32 = 10_000;
/// Largest entity filter accepted from callers.
const MAX_FILTER_ENTITIES: usize = 4096;

/// Reads up to `max_events` events after `cursor`. `entities_ptr`/`entities_len` optionally
/// restrict the feed to some entities (`entities_len == 0` means all).
#[no_mangle]
pub extern "C" fn adb_read_changes_json(
    db: *mut AdbDatabaseHandle,
    cursor: u64,
    max_events: u32,
    entities_ptr: *const u64,
    entities_len: usize,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let database = database(db)?;
        if max_events == 0 || max_events > MAX_EVENTS_PER_CALL {
            return Err(invalid(&format!(
                "max_events must be 1..={MAX_EVENTS_PER_CALL}"
            )));
        }
        let filter = if entities_len == 0 {
            ChangeFilter::all()
        } else {
            if entities_ptr.is_null() || entities_len > MAX_FILTER_ENTITIES {
                return Err(invalid("invalid entity filter"));
            }
            // SAFETY: non-null and bounded; caller guarantees `entities_len` readable values.
            ChangeFilter::entities(
                unsafe { slice::from_raw_parts(entities_ptr, entities_len) }
                    .iter()
                    .copied(),
            )
        };
        let batch = database
            .read_changes(ChangeCursor(Lsn(cursor)), max_events as usize, &filter)
            .map_err(map_db_error)?;
        let bytes = encode(batch).into_boxed_slice();
        init_out(out_json, Box::into_raw(Box::new(AdbBatchHandle { bytes })))?;
        Ok(AdbStatus::Ok)
    })
}

/// Writes the cursor at the durable end of the log (start of "new changes only").
#[no_mangle]
pub extern "C" fn adb_change_feed_end(
    db: *mut AdbDatabaseHandle,
    out_cursor: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_cursor, 0)?;
        let end = database(db)?.change_feed_end();
        init_out(out_cursor, end.0 .0)?;
        Ok(AdbStatus::Ok)
    })
}

/// Durably stores the cursor of a named consumer.
#[no_mangle]
pub extern "C" fn adb_commit_consumer_offset(
    db: *mut AdbDatabaseHandle,
    name_ptr: *const u8,
    name_len: usize,
    cursor: u64,
) -> AdbStatus {
    ffi_guard(|| {
        let database = database(db)?;
        let name = utf8(name_ptr, name_len, MAX_CONSUMER_NAME_BYTES)?;
        database
            .commit_consumer_offset(name, ChangeCursor(Lsn(cursor)))
            .map_err(map_db_error)?;
        Ok(AdbStatus::Ok)
    })
}

/// Reads the stored cursor of a named consumer; returns `NotFound` if it has none.
#[no_mangle]
pub extern "C" fn adb_consumer_offset(
    db: *mut AdbDatabaseHandle,
    name_ptr: *const u8,
    name_len: usize,
    out_cursor: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_cursor, 0)?;
        let database = database(db)?;
        let name = utf8(name_ptr, name_len, MAX_CONSUMER_NAME_BYTES)?;
        match database.consumer_offset(name) {
            Some(cursor) => {
                init_out(out_cursor, cursor.0 .0)?;
                Ok(AdbStatus::Ok)
            }
            None => Err((
                AdbStatus::NotFound,
                format!("no offset for consumer {name}"),
            )),
        }
    })
}

/// Encodes a batch, cutting it short (at an event boundary) if it would exceed the size limit.
fn encode(batch: ChangeBatch) -> Vec<u8> {
    let mut events = Vec::new();
    let mut size = 0usize;
    let mut next = batch.next;
    for event in &batch.events {
        let encoded = event_json(event);
        let event_size = encoded.to_string().len();
        if !events.is_empty() && size + event_size > MAX_CHANGES_JSON_BYTES {
            // Cut short: resume right after the last event actually returned.
            next = batch.events[events.len() - 1].cursor_after;
            break;
        }
        size += event_size;
        events.push(encoded);
    }
    json!({ "next_cursor": next.0 .0.to_string(), "events": events })
        .to_string()
        .into_bytes()
}

/// JSON form of one change event (see the module documentation).
fn event_json(event: &ChangeEvent) -> Value {
    json!({
        "tx_id": event.tx_id.0,
        "commit_ts": event.commit_ts.0,
        "commit_lsn": event.commit_lsn.0.to_string(),
        "cursor_after": event.cursor_after.0 .0.to_string(),
        "changes": event.catalog.iter().map(catalog_json).chain(event.changes.iter().map(|change| json!({
            "entity_id": change.entity_id(),
            "primary_key": change.primary_key().to_string(),
            "kind": match change.kind {
                ChangeKind::Insert => "insert",
                ChangeKind::Update => "update",
                ChangeKind::Delete => "delete",
            },
            "schema_version": change.schema_version.0,
            "before": change.before,
            "after": change.after,
        }))).collect::<Vec<_>>(),
    })
}

/// JSON form of one catalog change (see the module documentation).
fn catalog_json(change: &CatalogChange) -> Value {
    match change {
        CatalogChange::Schema { epoch, schema } => json!({
            "entity_id": schema.entity_id,
            "kind": "schema",
            "epoch": epoch.0,
            "schema": schema,
        }),
        CatalogChange::DropEntity { epoch, entity_id } => json!({
            "entity_id": entity_id,
            "kind": "drop_table",
            "epoch": epoch.0,
        }),
    }
}
