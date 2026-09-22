//! ABI and engine-version introspection functions.

/// Milestone 2 native ABI version.
pub const ABI_VERSION: u32 = 2;
/// Hardened engine version reported through the C ABI.
static ENGINE_VERSION: &[u8] = b"0.2.0+hardening.1";

/// Returns the exact native ABI version expected by the Milestone 2 JVM adapter.
#[no_mangle]
pub extern "C" fn adb_abi_version() -> u32 {
    ABI_VERSION
}

/// Returns a borrowed pointer to the static engine-version bytes and optionally writes their length.
#[no_mangle]
pub extern "C" fn adb_engine_version(out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = ENGINE_VERSION.len() };
    }
    ENGINE_VERSION.as_ptr()
}
