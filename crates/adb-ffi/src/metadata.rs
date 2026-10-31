//! Read-only C ABI metadata functions.

use crate::{error::ffi_guard, handle::AdbDatabaseHandle, AdbStatus};

/// Returns the latest committed MVCC timestamp known by the database.
#[no_mangle]
pub extern "C" fn adb_latest_committed_ts(
    db: *mut AdbDatabaseHandle,
    out_commit_ts: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        if out_commit_ts.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null commit output pointer".into(),
            ));
        }
        unsafe { *out_commit_ts = 0 };
        if db.is_null() {
            return Err((AdbStatus::InvalidArgument, "null database handle".into()));
        }
        let ts = unsafe { &*db }.database.latest_committed_ts();
        unsafe { *out_commit_ts = ts.0 };
        Ok(AdbStatus::Ok)
    })
}

/// Returns the current catalog epoch (plan wire v4: every plan carries the epoch it was made
/// at, and the engine refuses a plan of another epoch).
#[no_mangle]
pub extern "C" fn adb_current_schema_epoch(
    db: *mut AdbDatabaseHandle,
    out_epoch: *mut u64,
) -> AdbStatus {
    ffi_guard(|| {
        if out_epoch.is_null() {
            return Err((
                AdbStatus::InvalidArgument,
                "null epoch output pointer".into(),
            ));
        }
        unsafe { *out_epoch = 0 };
        if db.is_null() {
            return Err((AdbStatus::InvalidArgument, "null database handle".into()));
        }
        let epoch = unsafe { &*db }.database.schema_epoch();
        unsafe { *out_epoch = epoch.0 };
        Ok(AdbStatus::Ok)
    })
}
