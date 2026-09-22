//! Reader for the legacy single-file WAL format.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use adb_core::Lsn;
use crc32fast::Hasher;

use crate::{
    error::WalError,
    format::{HEADER_LEN, MAX_WAL_RECORD_BYTES, WAL_MAGIC, WAL_VERSION},
    record::WalRecord,
};

/// Sequential reader that validates WAL frames and tolerates only an incomplete crash suffix.
pub struct WalReader {
    file: File,
    offset: u64,
}

/// Implements bounded and checksum-validated single-file WAL reading.
impl WalReader {
    /// Opens an existing WAL file at offset zero.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        Ok(Self {
            file: File::open(path)?,
            offset: 0,
        })
    }

    /// Reads every complete valid frame into memory.
    pub fn read_all(mut self) -> Result<Vec<(Lsn, WalRecord)>, WalError> {
        let mut out = Vec::new();
        while let Some(item) = self.next_record()? {
            out.push(item);
        }
        Ok(out)
    }

    /// Returns the byte offset immediately following the last complete valid WAL frame.
    pub fn valid_end(mut self) -> Result<u64, WalError> {
        while self.next_record()?.is_some() {}
        Ok(self.offset)
    }

    /// Reads the next frame, returning `None` only for EOF or an incomplete final crash suffix.
    pub fn next_record(&mut self) -> Result<Option<(Lsn, WalRecord)>, WalError> {
        self.file.seek(SeekFrom::Start(self.offset))?;
        let frame_start = self.offset;

        let mut magic = [0u8; 4];
        match self.file.read_exact(&mut magic) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        if u32::from_le_bytes(magic) != WAL_MAGIC {
            return Err(WalError::Corrupt(format!(
                "invalid magic at offset {frame_start}"
            )));
        }

        let mut version = [0u8; 2];
        if !read_tail_field(&mut self.file, &mut version)? {
            return Ok(None);
        }
        let version = u16::from_le_bytes(version);
        if version != WAL_VERSION {
            return Err(WalError::Corrupt(format!(
                "unsupported WAL version {version}"
            )));
        }

        let mut len = [0u8; 4];
        if !read_tail_field(&mut self.file, &mut len)? {
            return Ok(None);
        }
        let payload_len = u32::from_le_bytes(len) as usize;
        if payload_len > MAX_WAL_RECORD_BYTES {
            return Err(WalError::Corrupt(format!(
                "payload length {payload_len} exceeds hardened limit at offset {frame_start}"
            )));
        }

        let mut crc = [0u8; 4];
        if !read_tail_field(&mut self.file, &mut crc)? {
            return Ok(None);
        }
        let expected_crc = u32::from_le_bytes(crc);

        let mut payload = vec![0u8; payload_len];
        if !read_tail_field(&mut self.file, &mut payload)? {
            return Ok(None);
        }
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        if hasher.finalize() != expected_crc {
            return Err(WalError::Corrupt(format!(
                "checksum mismatch at offset {frame_start}"
            )));
        }

        let record: WalRecord = bincode::deserialize(&payload)?;
        self.offset = frame_start
            .checked_add((HEADER_LEN + payload_len) as u64)
            .ok_or_else(|| WalError::Corrupt("WAL offset overflow".into()))?;
        Ok(Some((Lsn(frame_start), record)))
    }
}

/// Reads one tail field and maps a short final read to a recoverable incomplete suffix.
fn read_tail_field(file: &mut File, bytes: &mut [u8]) -> Result<bool, WalError> {
    match file.read_exact(bytes) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}
