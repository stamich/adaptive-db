//! Access to library-owned byte buffers (record batches and JSON documents).
use crate::{error::ffi_guard, handle::AdbBatchHandle, AdbStatus};

/// Borrowed pointer to the buffer bytes; valid until `adb_batch_release`.
#[no_mangle]
pub extern "C" fn adb_batch_data(batch: *const AdbBatchHandle) -> *const u8 {
    if batch.is_null() {
        return std::ptr::null();
    }
    unsafe { (&*batch).bytes.as_ptr() }
}

/// Buffer length in bytes (0 for a null handle).
#[no_mangle]
pub extern "C" fn adb_batch_len(batch: *const AdbBatchHandle) -> usize {
    if batch.is_null() {
        return 0;
    }
    unsafe { (&*batch).bytes.len() }
}

/// Frees a buffer exactly once.
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
