//! Checkpoint record: how far the projections reflect the canonical log.
//!
//! Format 3 (Milestone 2.0.3) is published through the checkpoint journal together with the
//! pages it describes, so it can never disagree with them. Format 2 (Milestones 1.6–2.0.2,
//! where every commit was forced to the stores) is still readable and migrated on load.

use std::path::{Path, PathBuf};

use adb_core::{CommitTs, Lsn};
use adb_journal::{envelope, replace_file, sync_dir, FileWrite};
use serde::{Deserialize, Serialize};

use crate::StorageError;

/// Current checkpoint payload format.
pub const CHECKPOINT_FORMAT_VERSION: u32 = 3;
/// Magic prefix of the checkpoint envelope.
const MAGIC: &[u8; 8] = b"ADBCK161";
/// Envelope version.
const ENVELOPE_VERSION: u16 = 1;
/// Upper bound on the checkpoint file size.
const MAX_CHECKPOINT_BYTES: usize = 4096;

/// Persisted progress of the projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Payload format (always [`CHECKPOINT_FORMAT_VERSION`] once loaded).
    pub format_version: u32,
    /// Log position at which recovery starts reading.
    pub replay_from: Lsn,
    /// Commits at or before this log position are already in the projections and are skipped
    /// even if recovery reads them (needed only for databases migrated from format 2).
    pub applied_through: Option<Lsn>,
    /// Highest committed timestamp reflected in the projections.
    pub last_commit_ts: CommitTs,
    /// Highest transaction id observed.
    pub last_tx_id: u64,
    /// Tombstones committed at or before this timestamp may have been vacuumed from the current
    /// store; scans at older snapshots must also consult the version index.
    pub vacuumed_through: CommitTs,
}

impl Default for Checkpoint {
    /// Empty database: replay from the start of the log.
    fn default() -> Self {
        Self {
            format_version: CHECKPOINT_FORMAT_VERSION,
            replay_from: Lsn(0),
            applied_through: None,
            last_commit_ts: CommitTs(0),
            last_tx_id: 0,
            vacuumed_through: CommitTs(0),
        }
    }
}

/// Format 2 payload of Milestones 1.6–2.0.2.
#[derive(Deserialize)]
struct CheckpointV2 {
    /// Always 2.
    format_version: u32,
    /// Last commit LSN applied to the current store.
    current_applied_lsn: u64,
    /// Last commit LSN applied to the version store.
    version_applied_lsn: u64,
    /// Highest commit timestamp.
    last_commit_ts: u64,
    /// Highest transaction id.
    last_tx_id: u64,
}

impl CheckpointV2 {
    /// Format 2 stores were forced per commit, so the whole log is re-read and every commit
    /// up to the older of the two frontiers is skipped.
    fn migrate(self) -> Checkpoint {
        let applied = self.current_applied_lsn.min(self.version_applied_lsn);
        Checkpoint {
            format_version: CHECKPOINT_FORMAT_VERSION,
            replay_from: Lsn(0),
            applied_through: (applied > 0).then_some(Lsn(applied)),
            last_commit_ts: CommitTs(self.last_commit_ts),
            last_tx_id: self.last_tx_id,
            vacuumed_through: CommitTs(0),
        }
    }
}

/// The checkpoint file of one database.
pub struct CheckpointStore {
    /// Path of `checkpoint.meta`.
    path: PathBuf,
}

impl CheckpointStore {
    /// Binds the store to `path`.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Loads the checkpoint; a missing file means an empty database.
    pub fn load(&self) -> Result<Checkpoint, StorageError> {
        if !self.path.exists() {
            return Ok(Checkpoint::default());
        }
        let bytes = std::fs::read(&self.path)?;
        let payload = if bytes.starts_with(MAGIC) {
            envelope::open(MAGIC, ENVELOPE_VERSION, &bytes, MAX_CHECKPOINT_BYTES)
                .map_err(StorageError::Invalid)?
        } else {
            &bytes[..] // pre-envelope Milestone 1.6 file
        };
        decode(payload)
    }

    /// Journal write that publishes `checkpoint` atomically with the pages it describes.
    pub fn journal_write(&self, checkpoint: &Checkpoint) -> Result<FileWrite, StorageError> {
        Ok(FileWrite::replace(&self.path, encode(checkpoint)?))
    }

    /// Publishes `checkpoint` directly (tools and tests).
    pub fn save(&self, checkpoint: &Checkpoint) -> Result<(), StorageError> {
        replace_file(&self.path, &encode(checkpoint)?)?;
        if let Some(parent) = self.path.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    }
}

/// Envelope-framed encoding; refuses any format other than the current one.
fn encode(checkpoint: &Checkpoint) -> Result<Vec<u8>, StorageError> {
    if checkpoint.format_version != CHECKPOINT_FORMAT_VERSION {
        return Err(StorageError::Invalid(
            "refusing to persist an unsupported checkpoint format".into(),
        ));
    }
    Ok(envelope::seal(
        MAGIC,
        ENVELOPE_VERSION,
        &bincode::serialize(checkpoint)?,
    ))
}

/// Decodes format 3, or format 2 with migration.
fn decode(payload: &[u8]) -> Result<Checkpoint, StorageError> {
    let version = payload
        .get(..4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .ok_or_else(|| StorageError::Invalid("truncated checkpoint".into()))?;
    match version {
        CHECKPOINT_FORMAT_VERSION => Ok(bincode::deserialize(payload)?),
        2 => {
            let legacy: CheckpointV2 = bincode::deserialize(payload)?;
            debug_assert_eq!(legacy.format_version, 2);
            Ok(legacy.migrate())
        }
        other => Err(StorageError::Invalid(format!(
            "unsupported checkpoint format {other}"
        ))),
    }
}

/// Unit tests of checkpoint persistence and migration.
#[cfg(test)]
mod tests {
    use super::*;

    /// A format-3 checkpoint round-trips; a missing file is the default.
    #[test]
    fn round_trips_format_3() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path().join("checkpoint.meta"));
        assert_eq!(store.load().unwrap(), Checkpoint::default());
        let checkpoint = Checkpoint {
            replay_from: Lsn(4096),
            last_commit_ts: CommitTs(9),
            last_tx_id: 12,
            vacuumed_through: CommitTs(3),
            ..Checkpoint::default()
        };
        store.save(&checkpoint).unwrap();
        assert_eq!(store.load().unwrap(), checkpoint);
    }

    /// A Milestone 2.0.2 checkpoint is migrated to "replay the whole log, skip applied commits".
    #[test]
    fn migrates_format_2() {
        /// Field-for-field image of the format-2 payload.
        #[derive(Serialize)]
        struct V2(u32, u64, u64, u64, u64);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.meta");
        let payload = bincode::serialize(&V2(2, 500, 400, 7, 8)).unwrap();
        std::fs::write(&path, envelope::seal(MAGIC, ENVELOPE_VERSION, &payload)).unwrap();

        let migrated = CheckpointStore::new(&path).load().unwrap();
        assert_eq!(migrated.replay_from, Lsn(0));
        assert_eq!(migrated.applied_through, Some(Lsn(400)));
        assert_eq!(migrated.last_commit_ts, CommitTs(7));
        assert_eq!(migrated.last_tx_id, 8);
    }

    /// Unknown formats are rejected.
    #[test]
    fn rejects_unknown_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.meta");
        let payload = bincode::serialize(&99u32).unwrap();
        std::fs::write(&path, envelope::seal(MAGIC, ENVELOPE_VERSION, &payload)).unwrap();
        assert!(CheckpointStore::new(&path).load().is_err());
    }
}
