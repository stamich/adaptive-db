//! Durable projection checkpoints.
//!
//! The registry's persistent part — each projection's descriptor, state and frontier, and the
//! registry's metadata epoch — lives in one small file, replaced atomically: the new content is
//! written to a temporary file, fsynced, renamed over the old one and the directory is fsynced.
//! A reader therefore sees the old or the new checkpoint, never a mixture. The content is
//! framed `magic | version | length | crc32 | payload`, so any damage is detected on load.
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use adb_core::ProjectionEpoch;
use serde::{Deserialize, Serialize};

use crate::{
    error::ProjectionError,
    model::{ProjectionDescriptor, ProjectionFrontier, ProjectionState},
};

/// File name of the checkpoint inside its directory.
pub const CHECKPOINT_FILE: &str = "projections.meta";
/// Magic bytes at the start of the file.
const MAGIC: &[u8; 8] = b"ADBPROJ1";
/// Version of the file layout.
const FORMAT_VERSION: u32 = 1;
/// Size of the fixed header: magic, version, length, checksum.
const HEADER_LEN: usize = 8 + 4 + 4 + 4;
/// Largest accepted payload (16 MiB); the real size is a few hundred bytes per projection.
const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
/// Most projections one checkpoint may hold.
const MAX_ENTRIES: usize = 4096;

/// The persistent state of one projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointEntry {
    /// Definition of the projection.
    pub descriptor: ProjectionDescriptor,
    /// State when the checkpoint was taken.
    pub state: ProjectionState,
    /// Frontier when the checkpoint was taken.
    pub frontier: ProjectionFrontier,
}

/// Everything the registry persists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionCheckpoint {
    /// Metadata epoch of the registry: moves on every registration, removal and definition or
    /// state change.
    pub epoch: ProjectionEpoch,
    /// The projections, in registration order.
    pub entries: Vec<CheckpointEntry>,
}

/// Reads and atomically replaces the checkpoint file of one directory.
#[derive(Debug, Clone)]
pub struct ProjectionCheckpointStore {
    /// Directory holding the file.
    dir: PathBuf,
}

impl ProjectionCheckpointStore {
    /// A store for the checkpoint in `dir` (created on first save).
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    /// Path of the checkpoint file.
    pub fn path(&self) -> PathBuf {
        self.dir.join(CHECKPOINT_FILE)
    }

    /// Path of the temporary file used while saving.
    fn temp_path(&self) -> PathBuf {
        self.dir.join(format!("{CHECKPOINT_FILE}.tmp"))
    }

    /// Loads the checkpoint; a missing file is an empty checkpoint.
    pub fn load(&self) -> Result<ProjectionCheckpoint, ProjectionError> {
        // A leftover temporary file is an interrupted save: the real file is still whole.
        let _ = fs::remove_file(self.temp_path());
        let bytes = match fs::read(self.path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProjectionCheckpoint::default())
            }
            Err(error) => return Err(error.into()),
        };
        decode(&bytes)
    }

    /// Atomically replaces the checkpoint with `checkpoint`.
    pub fn save(&self, checkpoint: &ProjectionCheckpoint) -> Result<(), ProjectionError> {
        fs::create_dir_all(&self.dir)?;
        let bytes = encode(checkpoint)?;
        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(self.temp_path())?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(self.temp_path(), self.path())?;
        #[cfg(unix)]
        File::open(&self.dir)?.sync_all()?;
        Ok(())
    }
}

/// Serialises and frames a checkpoint.
fn encode(checkpoint: &ProjectionCheckpoint) -> Result<Vec<u8>, ProjectionError> {
    if checkpoint.entries.len() > MAX_ENTRIES {
        return Err(ProjectionError::Invalid(format!(
            "more than {MAX_ENTRIES} projections"
        )));
    }
    let payload = bincode::serialize(checkpoint)
        .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(ProjectionError::Invalid("checkpoint too large".into()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Validates the frame and decodes the checkpoint.
fn decode(bytes: &[u8]) -> Result<ProjectionCheckpoint, ProjectionError> {
    let corrupt = |reason: &str| ProjectionError::Corrupt(reason.to_string());
    if bytes.len() < HEADER_LEN {
        return Err(corrupt("shorter than its header"));
    }
    if &bytes[..8] != MAGIC {
        return Err(corrupt("bad magic"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        return Err(ProjectionError::Corrupt(format!(
            "unsupported format version {version}"
        )));
    }
    let length = u32::from_le_bytes(bytes[12..16].try_into().expect("4 bytes")) as usize;
    let crc = u32::from_le_bytes(bytes[16..20].try_into().expect("4 bytes"));
    if length > MAX_PAYLOAD_BYTES || bytes.len() != HEADER_LEN + length {
        return Err(corrupt("length does not match the file"));
    }
    let payload = &bytes[HEADER_LEN..];
    if crc32fast::hash(payload) != crc {
        return Err(corrupt("checksum mismatch"));
    }
    let checkpoint: ProjectionCheckpoint = bincode::deserialize(payload)
        .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
    if checkpoint.entries.len() > MAX_ENTRIES {
        return Err(corrupt("too many projections"));
    }
    for entry in &checkpoint.entries {
        entry.descriptor.validate()?;
        if !entry.frontier.is_valid() {
            return Err(corrupt("frontier with the delta behind the base"));
        }
    }
    Ok(checkpoint)
}
