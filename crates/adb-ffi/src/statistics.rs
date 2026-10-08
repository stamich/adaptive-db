//! Optimizer statistics (ABI 5): `ANALYZE`, reading a statistics document and staleness.

use adb_engine::{AnalyzeOptions, TableStatistics};

use crate::handle::AdbDatabaseHandle;
use crate::{
    args::{bytes, database, init_out, FfiError},
    error::{ffi_guard, map_db_error, map_stats_error},
    handle::AdbBatchHandle,
    AdbStatus,
};

/// Maximum `ANALYZE` options JSON accepted by the C ABI.
pub const MAX_ANALYZE_OPTIONS_BYTES: usize = 64 * 1024;

/// Analyzes `entity_id` and returns its new statistics document as a UTF-8 JSON buffer (read
/// with `adb_batch_data`/`adb_batch_len`, free with `adb_batch_release`).
///
/// `options_len == 0` uses the default options (`options_ptr` may then be null).
#[no_mangle]
pub extern "C" fn adb_analyze_entity_json(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    options_ptr: *const u8,
    options_len: usize,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        let database = database(db)?;
        let options = if options_len == 0 {
            AnalyzeOptions::default()
        } else {
            let json = bytes(options_ptr, options_len, MAX_ANALYZE_OPTIONS_BYTES)?;
            AnalyzeOptions::from_json(json).map_err(map_stats_error)?
        };
        let statistics = database
            .analyze(entity_id, &options)
            .map_err(map_db_error)?;
        write_document(&statistics, out_json)
    })
}

/// Returns the statistics document of `entity_id` as a UTF-8 JSON buffer, or
/// `ADB_NOT_FOUND` when the entity was never analyzed.
#[no_mangle]
pub extern "C" fn adb_statistics_json(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    out_json: *mut *mut AdbBatchHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_json, std::ptr::null_mut())?;
        match database(db)?.statistics(entity_id) {
            Some(statistics) => write_document(&statistics, out_json),
            None => Err((
                AdbStatus::NotFound,
                format!("entity {entity_id} has no statistics; run ANALYZE"),
            )),
        }
    })
}

/// Writes the number of row mutations committed to `entity_id` since its last `ANALYZE`
/// (since creation if it was never analyzed).
#[no_mangle]
pub extern "C" fn adb_modifications_since_analyze(
    db: *mut AdbDatabaseHandle,
    entity_id: u64,
    out_count: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_count, 0)?;
        let count = database(db)?.modifications_since_analyze(entity_id);
        init_out(out_count, count)?;
        Ok(AdbStatus::Ok)
    })
}

/// Serializes `statistics` into a library-owned buffer stored in `out_json`.
fn write_document(
    statistics: &TableStatistics,
    out_json: *mut *mut AdbBatchHandle,
) -> Result<AdbStatus, FfiError> {
    let bytes = statistics
        .to_json()
        .map_err(map_stats_error)?
        .into_boxed_slice();
    init_out(out_json, Box::into_raw(Box::new(AdbBatchHandle { bytes })))?;
    Ok(AdbStatus::Ok)
}
