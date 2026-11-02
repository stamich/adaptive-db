//! The projection registry over the C ABI (ABI 8).
//!
//! The current-row store and the secondary indexes are registered by the engine and cannot be
//! changed from outside. External consumers of the change log (for example a search or analytics
//! feed) register an *asynchronous* projection by name, report how far they have consumed with
//! `adb_projection_advance`, and readers wait for them with `adb_projection_wait_until`.
//!
//! Documents are UTF-8 JSON in an `AdbBatchHandle`:
//!
//! ```json
//! {"descriptor":{"id":1000000,"name":"search","kind":"TestAsync","mode":"Asynchronous",
//!                "entity_id":3,"epoch":1,"supports_delta_repair":false},
//!  "state":"Ready","frontier":{"base_commit_seq":41,"delta_commit_seq":41},
//!  "head_commit_seq":43,"lag_commits":2,"lag_changes":0,"lag_bytes":0,"commits_per_second":0.0}
//! ```
//!
//! `adb_projection_list_json` returns an array of such documents ordered by id. State codes of
//! `adb_projection_set_state`: 0 building, 1 ready, 2 needs rebuild, 3 failed. Outcome codes of
//! `adb_projection_wait_until`: 0 reached, 1 timed out, 2 not readable (failed or needs a
//! rebuild), 3 unknown projection.
use std::time::Duration;

use adb_core::{CommitSeq, ProjectionId};
use adb_projection_runtime::{ProjectionMode, ProjectionState, WaitOutcome, MAX_WAIT};
use serde::Deserialize;

use crate::{
    args::{bytes, database, init_out, invalid},
    error::{ffi_guard, map_db_error},
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};

/// Largest registration document accepted from callers.
const MAX_REGISTRATION_BYTES: usize = 4 * 1024;

/// Registration request of an external consumer.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    /// Unique projection name.
    name: String,
    /// Entity the projection covers, if any.
    #[serde(default)]
    entity_id: Option<u64>,
}

/// Registers (or finds, by name) an asynchronous projection from `{"name":..,"entity_id":..}`
/// and writes its id. A new projection starts in the building state at commit 0.
#[no_mangle]
pub extern "C" fn adb_projection_register_json(
    db: *mut AdbDatabaseHandle,
    request_ptr: *const u8,
    request_len: usize,
    out_projection_id: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_projection_id, 0)?;
        let database = database(db)?;
        let request: Registration =
            serde_json::from_slice(bytes(request_ptr, request_len, MAX_REGISTRATION_BYTES)?)
                .map_err(|error| invalid(&error.to_string()))?;
        let id = database
            .register_consumer_projection(&request.name, request.entity_id)
            .map_err(map_db_error)?;
        init_out(out_projection_id, id.0)?;
        Ok(AdbStatus::Ok)
    })
}

/// Every projection with its state, frontier and lag, as a JSON array ordered by id.
#[no_mangle]
pub extern "C" fn adb_projection_list_json(
    db: *mut AdbDatabaseHandle,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let list = database(db)?.projection_registry().list();
        publish(out_json, serde_json::to_vec(&list))
    })
}

/// One projection as a JSON document; `ADB_NOT_FOUND` when it is not registered.
#[no_mangle]
pub extern "C" fn adb_projection_get_json(
    db: *mut AdbDatabaseHandle,
    projection_id: u64,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let progress = database(db)?
            .projection_registry()
            .get(ProjectionId(projection_id))
            .ok_or_else(|| not_found(projection_id))?;
        publish(out_json, serde_json::to_vec(&progress))
    })
}

/// Reports that an external projection consumed the change log through `commit_seq`. Frontiers
/// move forward only and cannot pass the newest published commit.
#[no_mangle]
pub extern "C" fn adb_projection_advance(
    db: *mut AdbDatabaseHandle,
    projection_id: u64,
    commit_seq: u64,
) -> AdbStatus {
    ffi_guard(|| {
        let database = database(db)?;
        external(database, projection_id)?;
        if commit_seq > database.latest_commit_seq().0 {
            return Err(invalid("commit_seq is beyond the newest published commit"));
        }
        database
            .projection_registry()
            .advance(ProjectionId(projection_id), CommitSeq(commit_seq))
            .map_err(|error| invalid(&error.to_string()))?;
        Ok(AdbStatus::Ok)
    })
}

/// Changes the state of an external projection (codes in the module documentation).
#[no_mangle]
pub extern "C" fn adb_projection_set_state(
    db: *mut AdbDatabaseHandle,
    projection_id: u64,
    state: u32,
) -> AdbStatus {
    ffi_guard(|| {
        let database = database(db)?;
        external(database, projection_id)?;
        let state = match state {
            0 => ProjectionState::Building,
            1 => ProjectionState::Ready,
            2 => ProjectionState::NeedsRebuild,
            3 => ProjectionState::Failed,
            other => return Err(invalid(&format!("unknown projection state {other}"))),
        };
        let registry = database.projection_registry();
        if state == ProjectionState::Building {
            registry.begin_rebuild(ProjectionId(projection_id))
        } else {
            registry.set_state(ProjectionId(projection_id), state)
        }
        .map_err(|error| invalid(&error.to_string()))?;
        Ok(AdbStatus::Ok)
    })
}

/// Blocks until the projection is ready and has consumed `commit_seq`, for at most `timeout_ms`
/// (0..=60000), and writes the outcome code (module documentation). The call itself succeeds
/// whatever the outcome.
#[no_mangle]
pub extern "C" fn adb_projection_wait_until(
    db: *mut AdbDatabaseHandle,
    projection_id: u64,
    commit_seq: u64,
    timeout_ms: u32,
    out_outcome: *mut u32,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_outcome, 3)?;
        let database = database(db)?;
        if u64::from(timeout_ms) > MAX_WAIT.as_millis() as u64 {
            return Err(invalid(&format!(
                "timeout_ms must be at most {}",
                MAX_WAIT.as_millis()
            )));
        }
        let outcome = database.projection_registry().wait_until(
            ProjectionId(projection_id),
            CommitSeq(commit_seq),
            Duration::from_millis(u64::from(timeout_ms)),
        );
        init_out(
            out_outcome,
            match outcome {
                WaitOutcome::Reached => 0,
                WaitOutcome::TimedOut => 1,
                WaitOutcome::NotReadable(_) => 2,
                WaitOutcome::Unknown => 3,
            },
        )?;
        Ok(AdbStatus::Ok)
    })
}

/// Checks that `projection_id` exists and is asynchronous: the engine owns the others.
fn external(
    database: &adb_engine::Database,
    projection_id: u64,
) -> Result<(), (AdbStatus, String)> {
    let progress = database
        .projection_registry()
        .get(ProjectionId(projection_id))
        .ok_or_else(|| not_found(projection_id))?;
    if progress.descriptor.mode != ProjectionMode::Asynchronous {
        return Err(invalid(&format!(
            "projection {} is maintained by the engine",
            progress.descriptor.name
        )));
    }
    Ok(())
}

/// The error for an unknown projection id.
fn not_found(projection_id: u64) -> (AdbStatus, String) {
    (
        AdbStatus::NotFound,
        format!("projection {projection_id} is not registered"),
    )
}

/// Stores a serialized document in a new batch handle.
fn publish(
    out_json: *mut *mut AdbBatchHandle,
    document: Result<Vec<u8>, serde_json::Error>,
) -> Result<AdbStatus, (AdbStatus, String)> {
    let bytes = document
        .map_err(|error| (AdbStatus::Internal, error.to_string()))?
        .into_boxed_slice();
    init_out(out_json, Box::into_raw(Box::new(AdbBatchHandle { bytes })))?;
    Ok(AdbStatus::Ok)
}
