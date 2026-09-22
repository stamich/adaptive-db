//! Module `abi_v2` for `adb-ffi` in Adaptive DB Milestone 2.0.1.
use adb_ffi::{adb_abi_version, AdbStatus};

/// Documents `abi_is_v2` and its role in the hardened Milestone 2.0.1 implementation.
#[test]
fn abi_is_v2() {
    assert_eq!(adb_abi_version(), 2);
    assert_eq!(AdbStatus::NotFound as i32, 7);
}
