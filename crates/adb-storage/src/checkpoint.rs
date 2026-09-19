//! Crash-safe checkpoint metadata with framing, CRC, and legacy-format compatibility.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use adb_core::{CommitTs, Lsn};
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};

use crate::StorageError;

/// Logical payload format version introduced by Milestone 1.6.
pub const CHECKPOINT_FORMAT_VERSION: u32 = 2;
/// Hardened checkpoint envelope magic.
const MAGIC: &[u8; 8] = b"ADBCK161";
/// Hardened checkpoint envelope version.
const ENVELOPE_VERSION: u16 = 1;
/// Maximum accepted checkpoint-file size.
const MAX_CHECKPOINT_BYTES: usize = 4096;

/// Records independently durable Current/Version WAL frontiers and transaction counters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Payload format version.
    pub format_version: u32,
    /// Highest commit LSN materialized in Current Store.
    pub current_applied_lsn: u64,
    /// Highest commit LSN materialized in Version Store.
    pub version_applied_lsn: u64,
    /// Highest committed timestamp observed.
    pub last_commit_ts: u64,
    /// Highest transaction id observed.
    pub last_tx_id: u64,
}

/// Creates an empty checkpoint at the initial durable frontier.
impl Default for Checkpoint {
    /// Returns an initial checkpoint with both stores at LSN zero.
    fn default() -> Self {
        Self {
            format_version: CHECKPOINT_FORMAT_VERSION,
            current_applied_lsn: 0,
            version_applied_lsn: 0,
            last_commit_ts: 0,
            last_tx_id: 0,
        }
    }
}

/// Provides typed access to persisted checkpoint frontiers.
impl Checkpoint {
    /// Returns Current Store materialization LSN.
    pub fn current_lsn(&self) -> Lsn {
        Lsn(self.current_applied_lsn)
    }
    /// Returns Version Store materialization LSN.
    pub fn version_lsn(&self) -> Lsn {
        Lsn(self.version_applied_lsn)
    }
    /// Returns latest committed timestamp.
    pub fn commit_ts(&self) -> CommitTs {
        CommitTs(self.last_commit_ts)
    }
    /// Returns the earliest store frontier from which WAL replay remains necessary.
    pub fn replay_lsn(&self) -> Lsn {
        Lsn(self.current_applied_lsn.min(self.version_applied_lsn))
    }
}

/// Loads and atomically publishes one checkpoint file.
pub struct CheckpointStore {
    path: PathBuf,
}

/// Implements framed/legacy checkpoint parsing and publish-last persistence ordering.
impl CheckpointStore {
    /// Creates a checkpoint store at the supplied path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Loads framed metadata or the original Milestone 1.6 bare-bincode checkpoint.
    pub fn load(&self) -> Result<Checkpoint, StorageError> {
        if !self.path.exists() {
            return Ok(Checkpoint::default());
        }
        let bytes = fs::read(&self.path)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(StorageError::Invalid("checkpoint file too large".into()));
        }
        let checkpoint: Checkpoint = if bytes.starts_with(MAGIC) {
            if bytes.len() < 18 {
                return Err(StorageError::Invalid(
                    "truncated checkpoint envelope".into(),
                ));
            }
            let envelope_version = u16::from_le_bytes([bytes[8], bytes[9]]);
            if envelope_version != ENVELOPE_VERSION {
                return Err(StorageError::Invalid(format!(
                    "unsupported checkpoint envelope {envelope_version}"
                )));
            }
            let crc = u32::from_le_bytes(
                bytes[10..14]
                    .try_into()
                    .map_err(|_| StorageError::Invalid("bad checkpoint crc".into()))?,
            );
            let len = u32::from_le_bytes(
                bytes[14..18]
                    .try_into()
                    .map_err(|_| StorageError::Invalid("bad checkpoint length".into()))?,
            ) as usize;
            if len > MAX_CHECKPOINT_BYTES || bytes.len() != 18 + len {
                return Err(StorageError::Invalid("invalid checkpoint length".into()));
            }
            let payload = &bytes[18..];
            let mut hasher = Hasher::new();
            hasher.update(payload);
            if hasher.finalize() != crc {
                return Err(StorageError::Invalid("checkpoint checksum mismatch".into()));
            }
            bincode::deserialize(payload)?
        } else {
            bincode::deserialize(&bytes)?
        };
        if checkpoint.format_version != CHECKPOINT_FORMAT_VERSION {
            return Err(StorageError::Invalid(format!(
                "unsupported checkpoint format {}",
                checkpoint.format_version
            )));
        }
        Ok(checkpoint)
    }

    /// Writes temp -> fsync temp -> rename -> fsync parent directory.
    pub fn save(&self, checkpoint: Checkpoint) -> Result<(), StorageError> {
        if checkpoint.format_version != CHECKPOINT_FORMAT_VERSION {
            return Err(StorageError::Invalid(
                "refusing to persist unsupported checkpoint format".into(),
            ));
        }
        let payload = bincode::serialize(&checkpoint)?;
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let crc = hasher.finalize();
        let len = u32::try_from(payload.len())
            .map_err(|_| StorageError::Invalid("checkpoint payload too large".into()))?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(MAGIC)?;
            file.write_all(&ENVELOPE_VERSION.to_le_bytes())?;
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
