//! C ABI database open/close operations with bounded input lengths.
use crate::{
    error::{ffi_guard, map_db_error, MAX_PATH_BYTES},
    handle::AdbDatabaseHandle,
    AdbStatus,
};
use adb_engine::Database;
use std::{path::PathBuf, slice};
/// Opens a database from a bounded UTF-8 filesystem path and returns an owned opaque handle.
#[no_mangle]
pub extern "C" fn adb_open(
    path_ptr: *const u8,
    path_len: usize,
    out_db: *mut *mut AdbDatabaseHandle,
) -> AdbStatus {
    ffi_guard(|| {
        if out_db.is_null() {
            return Err((AdbStatus::InvalidArgument, "null output pointer".into()));
        }
        unsafe {
            *out_db = std::ptr::null_mut();
        }
        if path_ptr.is_null() {
            return Err((AdbStatus::InvalidArgument, "null path pointer".into()));
        }
        if path_len == 0 || path_len > MAX_PATH_BYTES {
            return Err((
                AdbStatus::InvalidArgument,
                format!("path length must be 1..={MAX_PATH_BYTES}"),
            ));
        }
        let bytes = unsafe { slice::from_raw_parts(path_ptr, path_len) };
        let path =
            std::str::from_utf8(bytes).map_err(|e| (AdbStatus::InvalidArgument, e.to_string()))?;
        let db = Database::open(PathBuf::from(path)).map_err(map_db_error)?;
        unsafe {
            *out_db = Box::into_raw(Box::new(AdbDatabaseHandle { database: db }));
        }
        Ok(AdbStatus::Ok)
    })
}
/// Releases one valid database handle exactly once.
#[no_mangle]
pub extern "C" fn adb_close(handle: *mut AdbDatabaseHandle) -> AdbStatus {
    ffi_guard(|| {
        if handle.is_null() {
            return Err((AdbStatus::InvalidArgument, "null database handle".into()));
        }
        unsafe {
            drop(Box::from_raw(handle));
        }
        Ok(AdbStatus::Ok)
    })
}
