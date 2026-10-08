//! ABI and engine version introspection.

/// Native ABI version. 3 = Milestone 2.0.3 (CDC, checkpoint, vacuum, new status codes);
/// 4 = Milestone 2.1 (plan wire v2, batch format v2, query profiles, resource statuses);
/// 5 = Milestone 2.2.3 (ANALYZE, statistics documents, modification counters, profile node ids).
pub const ABI_VERSION: u32 = 5;
/// Engine version reported through the C ABI.
static ENGINE_VERSION: &[u8] = b"2.1.3";

/// Returns the ABI version; bindings must refuse to run against a different one.
#[no_mangle]
pub extern "C" fn adb_abi_version() -> u32 {
    ABI_VERSION
}

/// Returns static engine-version bytes and optionally writes their length.
#[no_mangle]
pub extern "C" fn adb_engine_version(out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        // SAFETY: non-null caller-owned slot.
        unsafe { *out_len = ENGINE_VERSION.len() };
    }
    ENGINE_VERSION.as_ptr()
}
