//! C ABI hardening regression tests.
use adb_ffi::{adb_execute_plan_json, adb_open, AdbStatus};
use std::ptr;
/// Verifies oversized path lengths are rejected before dereferencing the declared range.
#[test]
fn oversized_path_is_rejected_before_slice_creation() {
    let byte = b'x';
    let mut db = ptr::null_mut();
    assert_eq!(
        adb_open(&byte, usize::MAX, &mut db),
        AdbStatus::InvalidArgument
    );
    assert!(db.is_null());
}
/// Verifies oversized plan lengths are rejected and the output handle is cleared.
#[test]
fn oversized_plan_is_rejected_before_slice_creation() {
    let byte = b'{';
    let mut q = 1usize as *mut _;
    assert_eq!(
        adb_execute_plan_json(ptr::null_mut(), &byte, usize::MAX, &mut q),
        AdbStatus::InvalidArgument
    );
    assert!(q.is_null());
}
