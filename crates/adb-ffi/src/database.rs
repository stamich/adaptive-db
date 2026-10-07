//! Opening and closing a database.

use std::path::PathBuf;

use adb_engine::Database;

use crate::{
    args::{init_out, utf8},
    error::{ffi_guard, map_db_error, MAX_PATH_BYTES},
    handle::AdbDatabaseHandle,
    AdbStatus,
};

/// Opens (recovering if needed) the database at a UTF-8 path and returns an owned handle.
#[no_mangle]
pub extern "C" fn adb_open(
    path_ptr: *const u8,
    path_len: usize,
    out_db: *mut *mut AdbDatabaseHandle,
) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_db, std::ptr::null_mut())?;
        let path = utf8(path_ptr, path_len, MAX_PATH_BYTES)?;
        let database = Database::open(PathBuf::from(path)).map_err(map_db_error)?;
        init_out(
            out_db,
            Box::into_raw(Box::new(AdbDatabaseHandle { database })),
        )?;
        Ok(AdbStatus::Ok)
    })
}

/// Checkpoints (best effort) and releases a handle exactly once.
///
/// The handle is always released. A failed checkpoint is reported but loses nothing: the next
/// open replays the log.
#[no_mangle]
pub extern "C" fn adb_close(handle: *mut AdbDatabaseHandle) -> AdbStatus {
    ffi_guard(|| {
        if handle.is_null() {
            return Err((AdbStatus::InvalidArgument, "null database handle".into()));
        }
        // SAFETY: handle came from `adb_open` and is released exactly once (documented contract).
        let owned = unsafe { Box::from_raw(handle) };
        if owned.database.is_poisoned() {
            return Ok(AdbStatus::Ok);
        }
        owned.database.close().map_err(map_db_error)?;
        Ok(AdbStatus::Ok)
    })
}
