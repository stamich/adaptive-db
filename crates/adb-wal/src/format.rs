//! Constants of the log frame format.
/// Magic at the start of every frame.
pub const WAL_MAGIC: u32 = 0x4144_4257;
/// Frame format version.
pub const WAL_VERSION: u16 = 1;
/// Fixed frame header: magic, version, length, CRC.
pub const HEADER_LEN: usize = 4 + 2 + 4 + 4;
/// Largest accepted record payload.
pub const MAX_WAL_RECORD_BYTES: usize = 16 * 1024 * 1024;
