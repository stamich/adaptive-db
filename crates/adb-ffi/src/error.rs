//! Panic containment, thread-local error reporting, and engine-error status mapping for the C ABI.

use std::{cell::RefCell, panic::AssertUnwindSafe};

use adb_engine::DbError;

use crate::AdbStatus;

thread_local! {
    /// Stores the last FFI error bytes independently for each native caller thread.
    static LAST_ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Maximum UTF-8 database path accepted by the C ABI.
pub const MAX_PATH_BYTES: usize = 64 * 1024;
/// Maximum physical-plan JSON accepted by the C ABI.
pub const MAX_PLAN_JSON_BYTES: usize = 8 * 1024 * 1024;
/// Maximum mutation JSON accepted by INSERT/UPDATE C ABI calls.
pub const MAX_MUTATION_JSON_BYTES: usize = 8 * 1024 * 1024;

/// Replaces the current thread's last-error byte buffer.
pub fn set_last_error(message: impl Into<String>) {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = message.into().into_bytes());
}

/// Maps top-level engine errors into stable C status categories.
pub fn map_db_error(error: DbError) -> (AdbStatus, String) {
    let status = match &error {
        DbError::TransactionConflict => AdbStatus::Conflict,
        DbError::Io(_) => AdbStatus::IoError,
        DbError::Wal(adb_wal::WalError::Corrupt(_)) => AdbStatus::Corruption,
        DbError::Storage(adb_storage::StorageError::Invalid(_)) => AdbStatus::Corruption,
        _ => AdbStatus::Internal,
    };
    (status, error.to_string())
}

/// Executes one FFI operation while preventing Rust panics from unwinding across the C boundary.
pub fn ffi_guard(body: impl FnOnce() -> Result<AdbStatus, (AdbStatus, String)>) -> AdbStatus {
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(status)) => status,
        Ok(Err((status, message))) => {
            set_last_error(message);
            status
        }
        Err(_) => {
            set_last_error("panic contained at FFI boundary");
            AdbStatus::Internal
        }
    }
}

/// Returns the byte length of the current thread's last error message.
#[no_mangle]
pub extern "C" fn adb_last_error_len() -> usize {
    LAST_ERROR.with(|slot| slot.borrow().len())
}

/// Returns a borrowed pointer to the current thread's last error bytes.
///
/// The pointer is invalidated by the next FFI error on the same thread.
#[no_mangle]
pub extern "C" fn adb_last_error_ptr() -> *const u8 {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}
