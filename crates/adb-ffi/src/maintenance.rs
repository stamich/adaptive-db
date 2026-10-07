//! Checkpoint and vacuum.

use crate::{
    args::{database, init_out},
    error::{ffi_guard, map_db_error},
    handle::AdbDatabaseHandle,
    AdbStatus,
};

/// Persists all in-memory projection changes.
#[no_mangle]
pub extern "C" fn adb_checkpoint(db: *mut AdbDatabaseHandle) -> AdbStatus {
    ffi_guard(|| {
        database(db)?.checkpoint().map_err(map_db_error)?;
        Ok(AdbStatus::Ok)
    })
}

/// Removes expired delete tombstones; writes how many were removed.
#[no_mangle]
pub extern "C" fn adb_vacuum(db: *mut AdbDatabaseHandle, out_removed: *mut u64) -> AdbStatus {
    ffi_guard(|| {
        init_out(out_removed, 0)?;
        let report = database(db)?.vacuum().map_err(map_db_error)?;
        init_out(out_removed, report.tombstones_removed)?;
        Ok(AdbStatus::Ok)
    })
}
