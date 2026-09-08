//! Crash-safe persistent current-store checkpoint metadata.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use adb_core::{CommitTs, Lsn};
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};

use crate::StorageError;

/// Documents `CHECKPOINT_MAGIC` and its role in this hardened milestone.
const CHECKPOINT_MAGIC: &[u8; 8] = b"ADBCK101";
/// Documents `CHECKPOINT_VERSION` and its role in this hardened milestone.
const CHECKPOINT_VERSION: u16 = 1;

/// Records the WAL/commit frontier already materialized in the persistent current store.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Checkpoint {
    /// Commit-record LSN through which current state is durable.
    pub last_applied_commit_lsn: u64,
    /// Highest commit timestamp represented by the checkpoint.
    pub last_commit_ts: u64,
}

/// Provides typed accessors for checkpoint frontiers.
impl Checkpoint {
    /// Returns the checkpoint WAL LSN.
    pub fn lsn(&self) -> Lsn {
        Lsn(self.last_applied_commit_lsn)
    }
    /// Returns the checkpoint commit timestamp.
    pub fn commit_ts(&self) -> CommitTs {
        CommitTs(self.last_commit_ts)
    }
}

/// Loads and atomically replaces CRC-protected checkpoint metadata.
pub struct CheckpointStore {
    path: PathBuf,
}

/// Implements validated checkpoint load and crash-safe save.
impl CheckpointStore {
    /// Creates a checkpoint store at the supplied path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Loads and validates the checkpoint, returning the zero frontier when absent.
    pub fn load(&self) -> Result<Checkpoint, StorageError> {
        if !self.path.exists() {
            return Ok(Checkpoint::default());
        }
        let bytes = fs::read(&self.path)?;
        if bytes.len() < 18 || &bytes[..8] != CHECKPOINT_MAGIC {
            // Backward-compatible Milestone 1.5 checkpoint was exactly two serialized u64s.
            if bytes.len() != 16 {
                return Err(StorageError::Corrupt("bad checkpoint header".into()));
            }
            return Ok(bincode::deserialize(&bytes)?);
        }
        let version = u16::from_le_bytes(
            bytes[8..10]
                .try_into()
                .map_err(|_| StorageError::Corrupt("bad checkpoint version".into()))?,
        );
        if version != CHECKPOINT_VERSION {
            return Err(StorageError::Corrupt(format!(
                "unsupported checkpoint version {version}"
            )));
        }
        let expected = u32::from_le_bytes(
            bytes[10..14]
                .try_into()
                .map_err(|_| StorageError::Corrupt("bad checkpoint crc".into()))?,
        );
        let len = u32::from_le_bytes(
            bytes[14..18]
                .try_into()
                .map_err(|_| StorageError::Corrupt("bad checkpoint length".into()))?,
        ) as usize;
        if len > 1024 || bytes.len() != 18 + len {
            return Err(StorageError::Corrupt("invalid checkpoint length".into()));
        }
        let payload = &bytes[18..];
        let mut hasher = Hasher::new();
        hasher.update(payload);
        if hasher.finalize() != expected {
            return Err(StorageError::Corrupt("checkpoint checksum mismatch".into()));
        }
        Ok(bincode::deserialize(payload)?)
    }

    /// Persists a checkpoint using fsync-before-rename and directory fsync ordering.
    pub fn save(&self, checkpoint: Checkpoint) -> Result<(), StorageError> {
        let payload = bincode::serialize(&checkpoint)?;
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let crc = hasher.finalize();
        let len = u32::try_from(payload.len())
            .map_err(|_| StorageError::Corrupt("checkpoint payload too large".into()))?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(CHECKPOINT_MAGIC)?;
            file.write_all(&CHECKPOINT_VERSION.to_le_bytes())?;
            file.write_all(&crc.to_le_bytes())?;
            file.write_all(&len.to_le_bytes())?;
            file.write_all(&payload)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        if let Some(parent) = self.path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}
