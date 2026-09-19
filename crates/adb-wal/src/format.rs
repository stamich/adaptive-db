//! Shared constants for Adaptive DB WAL framing.
/// Magic value at the beginning of every WAL frame.
pub const WAL_MAGIC: u32 = 0x4144_4257;
/// WAL frame format version.
pub const WAL_VERSION: u16 = 1;
/// Number of bytes in the fixed WAL frame header.
pub const HEADER_LEN: usize = 4 + 2 + 4 + 4;
/// Maximum serialized WAL payload accepted from disk or append callers.
pub const MAX_WAL_RECORD_BYTES: usize = 16 * 1024 * 1024;
