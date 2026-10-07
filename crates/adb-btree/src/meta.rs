//! Persistence of a B+Tree's root page id.
//!
//! The root changes when the root node splits. Inside the engine the new root is published
//! through the checkpoint journal together with the pages it points at
//! ([`MetaStore::journal_write`]); [`MetaStore::save`] writes it directly for standalone use.

use std::path::{Path, PathBuf};

use adb_core::PageId;
use adb_journal::{envelope, replace_file, sync_dir, FileWrite};
use serde::{Deserialize, Serialize};

use crate::BTreeError;

const META_MAGIC: &[u8; 8] = b"ADBTM161";
const META_VERSION: u16 = 1;
const MAX_META_BYTES: usize = 1024;

#[derive(Debug, Serialize, Deserialize)]
struct BTreeMeta {
    root_page_id: u64,
}

/// Root metadata file of one tree.
pub struct MetaStore {
    path: PathBuf,
}

impl MetaStore {
    /// Binds the store to `path`.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Loads the persisted root, or `None` for a tree that was never created.
    ///
    /// The bare 8-byte bincode format of Milestone 1.5 is still accepted.
    pub fn load(&self) -> Result<Option<PageId>, BTreeError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.path)?;
        let payload = if bytes.len() == 8 {
            &bytes[..]
        } else {
            envelope::open(META_MAGIC, META_VERSION, &bytes, MAX_META_BYTES)
                .map_err(BTreeError::Corrupt)?
        };
        let meta: BTreeMeta = bincode::deserialize(payload)?;
        Ok(Some(PageId(meta.root_page_id)))
    }

    /// Journal write that publishes `root`.
    pub fn journal_write(&self, root: PageId) -> Result<FileWrite, BTreeError> {
        Ok(FileWrite::replace(&self.path, self.encode(root)?))
    }

    /// Publishes `root` directly (temp file, fsync, rename, directory fsync).
    pub fn save(&self, root: PageId) -> Result<(), BTreeError> {
        replace_file(&self.path, &self.encode(root)?)?;
        if let Some(parent) = self.path.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    }

    fn encode(&self, root: PageId) -> Result<Vec<u8>, BTreeError> {
        let payload = bincode::serialize(&BTreeMeta {
            root_page_id: root.0,
        })?;
        Ok(envelope::seal(META_MAGIC, META_VERSION, &payload))
    }
}
