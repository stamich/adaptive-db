//! Module `batch` for crate `adb-ffi`.
use crate::{error::ffi_guard, handle::AdbBatchHandle, AdbStatus};

/// Implements the `adb_batch_data` operation used by this subsystem.
#[no_mangle]
pub extern "C" fn adb_batch_data(batch: *const AdbBatchHandle) -> *const u8 {
    if batch.is_null() {
        return std::ptr::null();
    }
    unsafe { (&*batch).bytes.as_ptr() }
}

/// Implements the `adb_batch_len` operation used by this subsystem.
#[no_mangle]
pub extern "C" fn adb_batch_len(batch: *const AdbBatchHandle) -> usize {
    if batch.is_null() {
        return 0;
    }
    unsafe { (&*batch).bytes.len() }
}

/// Implements the `adb_batch_release` operation used by this subsystem.
#[no_mangle]
pub extern "C" fn adb_batch_release(batch: *mut AdbBatchHandle) -> AdbStatus {
    ffi_guard(|| {
        if batch.is_null() {
            return Err((AdbStatus::InvalidArgument, "null batch handle".to_string()));
        }
        unsafe { drop(Box::from_raw(batch)) };
        Ok(AdbStatus::Ok)
    })
}
