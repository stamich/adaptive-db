//! Writer module for the adb-wal crate.
//!
use std::{
    fs::{File, OpenOptions},
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

/// Appends framed write-ahead log records and provides the durability synchronization point.
pub struct WalWriter {
    file: File,
    next_lsn: u64,
}

/// Implements behavior for `WalWriter`.
impl WalWriter {
    /// Opens or creates the underlying resource and reconstructs the runtime state required by this subsystem.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let path = path.as_ref();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;

        let len = file.seek(SeekFrom::End(0))?;
        let valid_end = WalReader::open(path)?.valid_end()?;
        if valid_end < len {
            file.set_len(valid_end)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;

        Ok(Self {
            file,
            next_lsn: valid_end,
        })
    }

    /// Serializes and appends one WAL record frame and returns its starting LSN.
    pub fn append(&mut self, record: &WalRecord) -> Result<Lsn, WalError> {
        let payload = bincode::serialize(record)?;
        if payload.len() > MAX_WAL_RECORD_BYTES {
            return Err(WalError::RecordTooLarge(payload.len()));
        }
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| WalError::RecordTooLarge(payload.len()))?;

        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let checksum = hasher.finalize();

        let lsn = Lsn(self.next_lsn);

        self.file.write_all(&WAL_MAGIC.to_le_bytes())?;
        self.file.write_all(&WAL_VERSION.to_le_bytes())?;
        self.file.write_all(&payload_len.to_le_bytes())?;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&payload)?;

        self.next_lsn += (HEADER_LEN + payload.len()) as u64;

        Ok(lsn)
    }

    /// Flushes buffered WAL bytes and asks the operating system to synchronize file data.
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Returns the LSN at which the next WAL record would be appended.
    pub fn position(&self) -> Lsn {
        Lsn(self.next_lsn)
    }
}
