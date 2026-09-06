//! Format module for the adb-wal crate.
//!
/// Magic value that identifies the beginning of a WAL record frame.
pub const WAL_MAGIC: u32 = 0x4144_4257; // "ADBW"
/// On-disk WAL record format version understood by this milestone.
pub const WAL_VERSION: u16 = 1;
/// Encoded WAL frame header size in bytes.
pub const HEADER_LEN: usize = 4 + 2 + 4 + 4; // magic + version + payload len + crc32

/// Maximum serialized WAL payload accepted from disk or append callers.
pub const MAX_WAL_RECORD_BYTES: usize = 16 * 1024 * 1024;
