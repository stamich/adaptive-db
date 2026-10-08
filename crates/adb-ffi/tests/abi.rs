//! ABI identity and stable status codes.
use adb_ffi::{adb_abi_version, adb_engine_version, AdbStatus};

/// The ABI version, every status code and the engine version string are stable.
#[test]
fn abi_is_v5_and_status_codes_are_stable() {
    assert_eq!(adb_abi_version(), 5);
    let codes = [
        (AdbStatus::Ok, 0),
        (AdbStatus::EndOfStream, 1),
        (AdbStatus::InvalidArgument, 2),
        (AdbStatus::Conflict, 3),
        (AdbStatus::IoError, 4),
        (AdbStatus::Corruption, 5),
        (AdbStatus::Cancelled, 6),
        (AdbStatus::NotFound, 7),
        (AdbStatus::Poisoned, 8),
        (AdbStatus::LogTruncated, 9),
        (AdbStatus::ResourceLimit, 10),
        (AdbStatus::ArithmeticOverflow, 11),
        (AdbStatus::Internal, 255),
    ];
    for (status, code) in codes {
        assert_eq!(status as i32, code);
    }
    let mut len = 0;
    let ptr = adb_engine_version(&mut len);
    let version = unsafe { std::slice::from_raw_parts(ptr, len) };
    assert_eq!(version, b"2.1.3");
}
