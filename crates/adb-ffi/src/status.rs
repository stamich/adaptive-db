//! Module `status` for crate `adb-ffi`.
/// Enumerates `AdbStatus` alternatives used by this subsystem.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbStatus {
    Ok = 0,
    EndOfStream = 1,
    InvalidArgument = 2,
    Conflict = 3,
    IoError = 4,
    Corruption = 5,
    Cancelled = 6,
    Internal = 255,
}
