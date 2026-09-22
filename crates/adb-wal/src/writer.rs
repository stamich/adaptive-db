//! Writer for the legacy single-file WAL format.

use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use adb_core::Lsn;
use crc32fast::Hasher;

use crate::{
    error::WalError,
    format::{HEADER_LEN, MAX_WAL_RECORD_BYTES, WAL_MAGIC, WAL_VERSION},
    reader::WalReader,
    record::WalRecord,
};

/// Appends framed records to one WAL file and truncates only an incomplete crash tail on open.
pub struct WalWriter {
    file: std::fs::File,
    next_lsn: u64,
}

/// Implements durable append, synchronization, and crash-tail normalization.
impl WalWriter {
    /// Opens or creates the WAL and truncates an incomplete suffix after the last valid frame.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let path = path.as_ref();
        if !path.exists() {
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(path)?;
            return Ok(Self { file, next_lsn: 0 });
        }

        let valid_end = WalReader::open(path)?.valid_end()?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;
        let physical_end = file.seek(SeekFrom::End(0))?;
        if valid_end < physical_end {
            file.set_len(valid_end)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            next_lsn: valid_end,
        })
    }

    /// Appends one CRC-protected record and returns its starting LSN.
    pub fn append(&mut self, record: &WalRecord) -> Result<Lsn, WalError> {
        let payload = bincode::serialize(record)?;
        if payload.len() > MAX_WAL_RECORD_BYTES {
            return Err(WalError::RecordTooLarge(payload.len()));
        }
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| WalError::RecordTooLarge(payload.len()))?;
        let lsn = Lsn(self.next_lsn);
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let checksum = hasher.finalize();

        self.file.write_all(&WAL_MAGIC.to_le_bytes())?;
        self.file.write_all(&WAL_VERSION.to_le_bytes())?;
        self.file.write_all(&payload_len.to_le_bytes())?;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.next_lsn = self
            .next_lsn
            .checked_add((HEADER_LEN + payload.len()) as u64)
            .ok_or_else(|| WalError::Corrupt("WAL LSN overflow".into()))?;
        Ok(lsn)
    }

    /// Flushes process buffers and synchronizes WAL data to stable storage.
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Returns the next append LSN.
    pub fn position(&self) -> Lsn {
        Lsn(self.next_lsn)
    }
}
