//! Durable, named change-feed cursors ("consumer offsets").
//!
//! Stored in `cdc/offsets.meta` as a checksummed, atomically replaced map. Offsets are small
//! and updated by consumers, not by commits, so a direct atomic file replacement is enough.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adb_core::Lsn;
use adb_journal::{envelope, replace_file, sync_dir};
use adb_storage::StorageError;
use parking_lot::Mutex;

use crate::{cdc::ChangeCursor, DbError};

/// Magic prefix of the offsets file envelope.
const MAGIC: &[u8; 8] = b"ADBCDC01";
/// Envelope version of the offsets file.
const VERSION: u16 = 1;
/// Upper bound on the offsets file size.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Longest accepted consumer name in bytes.
pub const MAX_CONSUMER_NAME_BYTES: usize = 256;

/// Map of consumer name to its committed cursor.
pub struct ConsumerOffsets {
    /// Path of `cdc/offsets.meta`.
    path: PathBuf,
    /// Cursor (log position) of every named consumer.
    offsets: Mutex<BTreeMap<String, u64>>,
}

impl ConsumerOffsets {
    /// Loads the offsets of the database in `dir`.
    pub fn open(dir: &Path) -> Result<Self, DbError> {
        let cdc_dir = dir.join("cdc");
        std::fs::create_dir_all(&cdc_dir)?;
        let path = cdc_dir.join("offsets.meta");
        let offsets = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let payload =
                envelope::open(MAGIC, VERSION, &bytes, MAX_FILE_BYTES).map_err(corrupt)?;
            bincode::deserialize(payload).map_err(|error| corrupt(error.to_string()))?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path,
            offsets: Mutex::new(offsets),
        })
    }

    /// Committed cursor of `name`, if any.
    pub fn get(&self, name: &str) -> Option<ChangeCursor> {
        self.offsets
            .lock()
            .get(name)
            .map(|lsn| ChangeCursor(Lsn(*lsn)))
    }

    /// Durably records `cursor` for `name`.
    pub fn commit(&self, name: &str, cursor: ChangeCursor) -> Result<(), DbError> {
        if name.is_empty() || name.len() > MAX_CONSUMER_NAME_BYTES {
            return Err(DbError::InvalidArgument(format!(
                "consumer name must be 1..={MAX_CONSUMER_NAME_BYTES} bytes"
            )));
        }
        let mut offsets = self.offsets.lock();
        let mut updated = offsets.clone();
        updated.insert(name.to_string(), cursor.0 .0);
        let payload = bincode::serialize(&updated).map_err(|error| corrupt(error.to_string()))?;
        replace_file(&self.path, &envelope::seal(MAGIC, VERSION, &payload))?;
        if let Some(parent) = self.path.parent() {
            sync_dir(parent)?;
        }
        *offsets = updated;
        Ok(())
    }
}

/// Maps a malformed offsets file to a storage-corruption error.
fn corrupt(reason: String) -> DbError {
    DbError::Storage(StorageError::Invalid(format!("CDC offsets file: {reason}")))
}
