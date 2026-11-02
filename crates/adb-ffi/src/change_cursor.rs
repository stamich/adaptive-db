//! Streaming the change log over the C ABI (ABI 8).
//!
//! `adb_change_cursor_open` positions a cursor after a commit sequence number; each
//! `adb_change_cursor_next_batch` returns the next commits as one UTF-8 JSON document in an
//! `AdbBatchHandle` (read it with `adb_batch_data`/`adb_batch_len`, free it with
//! `adb_batch_release`), or `ADB_END_OF_STREAM` when the consumer is caught up:
//!
//! ```json
//! {"first_commit_seq":"5","last_commit_seq":"6","bytes":412,
//!  "commits":[
//!   {"commit_seq":"5","commit_ts":"5","schema_epoch":"2","tx_id":"9","kind":"data",
//!    "rows":[{"entity_id":1,"primary_key":"7","kind":"update",
//!             "before_schema_version":1,"after_schema_version":1,
//!             "before":{"fields":{"1":{"Int64":10}},"schema_version":1},
//!             "after":{"fields":{"1":{"Int64":20}},"schema_version":1}}]},
//!   {"commit_seq":"6","commit_ts":"6","schema_epoch":"3","tx_id":"10","kind":"schema",
//!    "changes":[{"kind":"schema","entity_id":1,"schema_version":2}]}]}
//! ```
//!
//! Sequence numbers, timestamps, epochs and transaction ids (`u64`) and primary keys (`i64`, the
//! SQL value) are decimal strings. A batch holds at most `max_commits` commits and about
//! `max_bytes` of encoded change data, and is cut short at a commit boundary when its JSON would
//! exceed the response limit; the cursor then resumes right after the last commit returned. At
//! least one commit is returned when one is available.
use adb_change_log::{ChangeEnvelope, ChangePayload, SchemaChangeMarker};
use adb_core::{CommitSeq, RowId};
use serde_json::{json, Value};

use crate::{
    args::{database, init_out, invalid},
    error::{ffi_guard, map_change_log_error, MAX_CHANGES_JSON_BYTES},
    handle::{AdbBatchHandle, AdbChangeCursorHandle, AdbDatabaseHandle},
    AdbStatus,
};

/// Largest `max_commits` accepted from callers.
const MAX_COMMITS_PER_CALL: u32 = 10_000;
/// Largest `max_bytes` accepted from callers.
const MAX_BYTES_PER_CALL: u64 = 64 * 1024 * 1024;

/// Opens a cursor that delivers the commits above `after_commit_seq` (0 = from the start of the
/// retained log). `ADB_LOG_TRUNCATED` when commits right above it were already pruned.
#[no_mangle]
pub extern "C" fn adb_change_cursor_open(
    db: *mut AdbDatabaseHandle,
    after_commit_seq: u64,
    out_cursor: *mut *mut AdbChangeCursorHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_cursor, std::ptr::null_mut())?;
        let cursor = database(db)?
            .change_log()
            .cursor(CommitSeq(after_commit_seq))
            .map_err(map_change_log_error)?;
        let handle = AdbChangeCursorHandle {
            cursor: parking_lot::Mutex::new(cursor),
        };
        init_out(out_cursor, Box::into_raw(Box::new(handle)))?;
        Ok(AdbStatus::Ok)
    })
}

/// Returns the next batch of commits, or `ADB_END_OF_STREAM` (and a null batch) when caught up.
#[no_mangle]
pub extern "C" fn adb_change_cursor_next_batch(
    cursor: *mut AdbChangeCursorHandle,
    max_commits: u32,
    max_bytes: u64,
    out_batch: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_batch, std::ptr::null_mut())?;
        if cursor.is_null() {
            return Err(invalid("null change cursor handle"));
        }
        if max_commits == 0 || max_commits > MAX_COMMITS_PER_CALL {
            return Err(invalid(&format!(
                "max_commits must be 1..={MAX_COMMITS_PER_CALL}"
            )));
        }
        if max_bytes == 0 || max_bytes > MAX_BYTES_PER_CALL {
            return Err(invalid(&format!(
                "max_bytes must be 1..={MAX_BYTES_PER_CALL}"
            )));
        }
        // SAFETY: a live handle from `adb_change_cursor_open` (documented contract).
        let mut guard = unsafe { &*cursor }.cursor.lock();
        let Some(batch) = guard
            .next_batch(max_commits as usize, max_bytes)
            .map_err(map_change_log_error)?
        else {
            return Ok(AdbStatus::EndOfStream);
        };
        let mut commits = Vec::new();
        let mut size = 0usize;
        let mut last = batch.first_commit_seq;
        for envelope in &batch.commits {
            let encoded = commit_json(envelope);
            let commit_size = encoded.to_string().len();
            if !commits.is_empty() && size + commit_size > MAX_CHANGES_JSON_BYTES {
                // Cut short: resume right after the last commit actually returned.
                guard.rewind_to(last);
                break;
            }
            size += commit_size;
            last = envelope.commit_seq;
            commits.push(encoded);
        }
        let document = json!({
            "first_commit_seq": batch.first_commit_seq.0.to_string(),
            "last_commit_seq": last.0.to_string(),
            "bytes": batch.bytes,
            "commits": commits,
        });
        let bytes = document.to_string().into_bytes().into_boxed_slice();
        init_out(out_batch, Box::into_raw(Box::new(AdbBatchHandle { bytes })))?;
        Ok(AdbStatus::Ok)
    })
}

/// Writes the last commit sequence number the cursor delivered (its starting point before the
/// first batch).
#[no_mangle]
pub extern "C" fn adb_change_cursor_position(
    cursor: *mut AdbChangeCursorHandle,
    out_commit_seq: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_commit_seq, 0)?;
        if cursor.is_null() {
            return Err(invalid("null change cursor handle"));
        }
        // SAFETY: a live handle from `adb_change_cursor_open` (documented contract).
        let position = unsafe { &*cursor }.cursor.lock().position();
        init_out(out_commit_seq, position.0)?;
        Ok(AdbStatus::Ok)
    })
}

/// Frees a cursor exactly once.
#[no_mangle]
pub extern "C" fn adb_change_cursor_close(cursor: *mut AdbChangeCursorHandle) -> AdbStatus {
    ffi_guard(|| {
        if cursor.is_null() {
            return Err(invalid("null change cursor handle"));
        }
        // SAFETY: a handle from `adb_change_cursor_open`, released once (documented contract).
        unsafe { drop(Box::from_raw(cursor)) };
        Ok(AdbStatus::Ok)
    })
}

/// JSON form of one commit (see the module documentation).
fn commit_json(envelope: &ChangeEnvelope) -> Value {
    let mut commit = json!({
        "commit_seq": envelope.commit_seq.0.to_string(),
        "commit_ts": envelope.commit_ts.0.to_string(),
        "schema_epoch": envelope.schema_epoch.0.to_string(),
    });
    let object = commit.as_object_mut().expect("an object");
    match &envelope.payload {
        ChangePayload::Data { tx_id, rows } => {
            object.insert("tx_id".into(), json!(tx_id.0.to_string()));
            object.insert("kind".into(), json!("data"));
            object.insert(
                "rows".into(),
                Value::Array(
                    rows.iter()
                        .map(|row| {
                            let kind = match (&row.before, &row.after) {
                                (None, _) => "insert",
                                (Some(_), Some(_)) => "update",
                                (Some(_), None) => "delete",
                            };
                            json!({
                                "entity_id": row.entity_id,
                                "primary_key": primary_key(row.row_id),
                                "kind": kind,
                                "before_schema_version": row.before_schema_version.map(|v| v.0),
                                "after_schema_version": row.after_schema_version.map(|v| v.0),
                                "before": row.before,
                                "after": row.after,
                            })
                        })
                        .collect(),
                ),
            );
        }
        ChangePayload::Schema { tx_id, changes } => {
            object.insert("tx_id".into(), json!(tx_id.0.to_string()));
            object.insert("kind".into(), json!("schema"));
            object.insert(
                "changes".into(),
                Value::Array(changes.iter().map(marker_json).collect()),
            );
        }
    }
    commit
}

/// The SQL primary key of a row as a decimal string.
fn primary_key(row_id: RowId) -> String {
    row_id.signed_primary_key().to_string()
}

/// JSON form of one catalog marker.
fn marker_json(marker: &SchemaChangeMarker) -> Value {
    match marker {
        SchemaChangeMarker::Schema {
            entity_id,
            schema_version,
        } => json!({"kind": "schema", "entity_id": entity_id, "schema_version": schema_version.0}),
        SchemaChangeMarker::DropEntity { entity_id } => {
            json!({"kind": "drop_table", "entity_id": entity_id})
        }
        SchemaChangeMarker::Epoch => json!({"kind": "epoch"}),
    }
}
