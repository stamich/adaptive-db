use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use adb_core::Lsn;
use crc32fast::Hasher;

use crate::{
    error::WalError,
    format::{WAL_MAGIC, WAL_VERSION},
    record::WalRecord,
};

pub struct WalWriter {
    file: File,
    next_lsn: u64,
}

impl WalWriter {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;

        let len = file.seek(SeekFrom::End(0))?;

        Ok(Self {
            file,
            next_lsn: len,
        })
    }

    pub fn append(&mut self, record: &WalRecord) -> Result<Lsn, WalError> {
        let payload = bincode::serialize(record)?;

        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let checksum = hasher.finalize();

        let lsn = Lsn(self.next_lsn);

        self.file.write_all(&WAL_MAGIC.to_le_bytes())?;
        self.file.write_all(&WAL_VERSION.to_le_bytes())?;
        self.file
            .write_all(&(payload.len() as u32).to_le_bytes())?;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&payload)?;

        self.next_lsn += (4 + 2 + 4 + 4 + payload.len()) as u64;

        Ok(lsn)
    }

    pub fn sync(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    pub fn position(&self) -> Lsn {
        Lsn(self.next_lsn)
    }
}
