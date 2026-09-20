//! Module `version` for crate `adb-ffi`.
/// Defines the `ABI_VERSION` constant used by this subsystem.
pub const ABI_VERSION: u32 = 1;
/// Defines the `ENGINE_VERSION` constant used by this subsystem.
static ENGINE_VERSION: &[u8] = b"0.1.7+hardening.1";

/// Implements the `adb_abi_version` operation used by this subsystem.
#[no_mangle]
pub extern "C" fn adb_abi_version() -> u32 {
    ABI_VERSION
}

/// Implements the `adb_engine_version` operation used by this subsystem.
#[no_mangle]
pub extern "C" fn adb_engine_version(out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = ENGINE_VERSION.len() };
    }
    ENGINE_VERSION.as_ptr()
}
