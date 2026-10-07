//! Validation of raw C ABI arguments, shared by every entry point.
use std::slice;

use adb_engine::Database;

use crate::{handle::AdbDatabaseHandle, AdbStatus};

/// Error type of every FFI body: a status plus the message stored in the last-error slot.
pub type FfiError = (AdbStatus, String);

/// Borrows the database behind a non-null handle.
pub fn database<'a>(db: *mut AdbDatabaseHandle) -> Result<&'a Database, FfiError> {
    if db.is_null() {
        return Err(invalid("null database handle"));
    }
    // SAFETY: the caller passes a live handle obtained from `adb_open` (documented contract).
    Ok(&unsafe { &*db }.database)
}

/// Initializes an output slot before any fallible work so callers never read garbage.
pub fn init_out<T>(out: *mut T, value: T) -> Result<(), FfiError> {
    if out.is_null() {
        return Err(invalid("null output pointer"));
    }
    // SAFETY: non-null, caller-owned, properly aligned output slot (documented contract).
    unsafe { out.write(value) };
    Ok(())
}

/// Borrows `len` bytes after checking the pointer and `1..=max` length.
pub fn bytes<'a>(ptr: *const u8, len: usize, max: usize) -> Result<&'a [u8], FfiError> {
    if ptr.is_null() {
        return Err(invalid("null input pointer"));
    }
    if len == 0 || len > max {
        return Err(invalid(&format!("input length must be 1..={max}")));
    }
    // SAFETY: non-null and bounded; the caller guarantees `len` readable bytes.
    Ok(unsafe { slice::from_raw_parts(ptr, len) })
}

/// Borrows a bounded UTF-8 string.
pub fn utf8<'a>(ptr: *const u8, len: usize, max: usize) -> Result<&'a str, FfiError> {
    std::str::from_utf8(bytes(ptr, len, max)?).map_err(|error| invalid(&error.to_string()))
}

/// An `InvalidArgument` error.
pub fn invalid(message: &str) -> FfiError {
    (AdbStatus::InvalidArgument, message.to_string())
}
