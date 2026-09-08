//! Crash-safe B+Tree root metadata persistence.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use adb_core::PageId;
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};

use crate::BTreeError;

/// Documents `META_MAGIC` and its role in this hardened milestone.
const META_MAGIC: &[u8; 8] = b"ADBTM101";
/// Documents `META_VERSION` and its role in this hardened milestone.
const META_VERSION: u16 = 1;

/// Serializable root metadata payload.
#[derive(Debug, Serialize, Deserialize)]
struct BTreeMeta {
    root_page_id: u64,
}

/// Persists and validates the current B+Tree root.
pub struct MetaStore {
    path: PathBuf,
}

/// Implements crash-safe root metadata load/save.
impl MetaStore {
    /// Creates a root metadata store at the supplied path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Loads and validates root metadata when present.
    pub fn load(&self) -> Result<Option<PageId>, BTreeError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)?;
        if bytes.len() < 18 || &bytes[..8] != META_MAGIC {
            // Backward-compatible Milestone 1.5 metadata was exactly one serialized u64.
            if bytes.len() != 8 {
                return Err(BTreeError::Corrupt("bad btree metadata header".into()));
            }
            let legacy: BTreeMeta = bincode::deserialize(&bytes)?;
            return Ok(Some(PageId(legacy.root_page_id)));
        }
        let version = u16::from_le_bytes(
            bytes[8..10]
                .try_into()
                .map_err(|_| BTreeError::Corrupt("bad metadata version".into()))?,
        );
        if version != META_VERSION {
            return Err(BTreeError::Corrupt(format!(
                "unsupported btree metadata version {version}"
            )));
        }
        let expected = u32::from_le_bytes(
            bytes[10..14]
                .try_into()
                .map_err(|_| BTreeError::Corrupt("bad metadata crc".into()))?,
        );
        let len = u32::from_le_bytes(
            bytes[14..18]
                .try_into()
                .map_err(|_| BTreeError::Corrupt("bad metadata length".into()))?,
        ) as usize;
        if len > 1024 || bytes.len() != 18 + len {
            return Err(BTreeError::Corrupt("invalid btree metadata length".into()));
        }
        let payload = &bytes[18..];
        let mut hasher = Hasher::new();
        hasher.update(payload);
        if hasher.finalize() != expected {
            return Err(BTreeError::Corrupt(
                "btree metadata checksum mismatch".into(),
            ));
        }
        let meta: BTreeMeta = bincode::deserialize(payload)?;
        Ok(Some(PageId(meta.root_page_id)))
    }

    /// Atomically publishes root metadata using file fsync, rename and directory fsync.
    pub fn save(&self, root: PageId) -> Result<(), BTreeError> {
        let payload = bincode::serialize(&BTreeMeta {
            root_page_id: root.0,
        })?;
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let crc = hasher.finalize();
        let len = u32::try_from(payload.len())
            .map_err(|_| BTreeError::Corrupt("metadata payload too large".into()))?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(META_MAGIC)?;
            file.write_all(&META_VERSION.to_le_bytes())?;
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
