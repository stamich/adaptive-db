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

pub struct WalReader {
    file: File,
    offset: u64,
}

impl WalReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let file = File::open(path)?;
        Ok(Self { file, offset: 0 })
    }
    
    pub fn read_all(mut self) -> Result<Vec<(Lsn, WalRecord)>, WalError> {
        let mut out = Vec::new();

        loop {
            match self.next_record()? {
                Some(item) => out.push(item),
                None => return Ok(out),
            }
        }
    }
    
    pub fn valid_end(mut self) -> Result<u64, WalError> {
        while self.next_record()?.is_some() {}
        Ok(self.offset)
    }
    
    pub fn next_record(&mut self) -> Result<Option<(Lsn, WalRecord)>, WalError> {
        self.file.seek(SeekFrom::Start(self.offset))?;

        let mut magic = [0u8; 4];
        match self.file.read_exact(&mut magic) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let magic = u32::from_le_bytes(magic);
        if magic != WAL_MAGIC {
            return Err(WalError::Corrupt(format!(
                "invalid magic at offset {}",
                self.offset
            )));
        }

        let mut version = [0u8; 2];
        if let Err(e) = self.file.read_exact(&mut version) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }

        let version = u16::from_le_bytes(version);
        if version != WAL_VERSION {
            return Err(WalError::Corrupt(format!(
                "unsupported WAL version {version}"
            )));
        }

        let mut len = [0u8; 4];
        if let Err(e) = self.file.read_exact(&mut len) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }
        let payload_len = u32::from_le_bytes(len) as usize;
        if payload_len > MAX_WAL_RECORD_BYTES {
            return Err(WalError::Corrupt(format!(
                "payload length {payload_len} exceeds hardened limit at offset {}",
                self.offset
            )));
        }

        let mut crc = [0u8; 4];
        if let Err(e) = self.file.read_exact(&mut crc) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }
        let expected_crc = u32::from_le_bytes(crc);

        let mut payload = vec![0u8; payload_len];
        if let Err(e) = self.file.read_exact(&mut payload) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                // Partial tail after crash: ignore safely.
                return Ok(None);
            }
            return Err(e.into());
        }

        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let actual_crc = hasher.finalize();

        if actual_crc != expected_crc {
            return Err(WalError::Corrupt(format!(
                "checksum mismatch at offset {}",
                self.offset
            )));
        }

        let record: WalRecord = bincode::deserialize(&payload)?;
        let lsn = Lsn(self.offset);

        self.offset += (HEADER_LEN + payload_len) as u64;

        Ok(Some((lsn, record)))
    }
}
