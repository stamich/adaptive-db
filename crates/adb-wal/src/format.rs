pub const WAL_MAGIC: u32 = 0x4144_4257; // "ADBW"
pub const WAL_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 4 + 2 + 4 + 4; // magic + version + payload len + crc32
