//! ABI and engine version introspection.

/// Native ABI version. 3 = Milestone 2.0.3 (CDC, checkpoint, vacuum, new status codes);
/// 4 = Milestone 2.1 (plan wire v2, batch format v2, query profiles, resource statuses);
/// 5 = Milestone 2.2.3 (ANALYZE, statistics documents, modification counters, profile node ids);
/// 6 = Milestone 2.3.0 (secondary indexes, plan wire v3, statistics format 2, order-preserving
/// primary keys, `ADB_INCOMPATIBLE_FORMAT`);
/// 7 = Milestone 2.4.0 (entity schemas and DDL, plan wire v4 with the catalog epoch,
/// `ADB_SCHEMA_CHANGED`, statistics format 3).
pub const ABI_VERSION: u32 = 7;
/// Engine version reported through the C ABI.
static ENGINE_VERSION: &[u8] = b"2.4.0";

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
