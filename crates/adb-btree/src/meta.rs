//! Crash-safe B+Tree root metadata persistence with legacy compatibility.
use crate::BTreeError;
use adb_core::PageId;
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
/// Defines the `META_MAGIC` constant used by this subsystem.
const META_MAGIC: &[u8; 8] = b"ADBTM161";
const META_VERSION: u16 = 1;
const MAX_META_BYTES: usize = 1024;
/// Serializable B+Tree root metadata payload.
#[derive(Debug, Serialize, Deserialize)]
struct BTreeMeta {
    root_page_id: u64,
}
/// Stores the current B+Tree root page id.
pub struct MetaStore {
    path: PathBuf,
}
/// Implements validated legacy/framed load and fsync-safe publication.
impl MetaStore {
    /// Creates a metadata store at the supplied path.
    /// Implements the `new` operation used by this subsystem.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
    /// Loads current root metadata, accepting the original bare-bincode format once.
    /// Implements the `load` operation used by this subsystem.
    pub fn load(&self) -> Result<Option<PageId>, BTreeError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)?;
        if bytes.len() > MAX_META_BYTES {
            return Err(BTreeError::Corrupt("btree metadata too large".into()));
        }
        if bytes.len() == 8 {
            let m: BTreeMeta = bincode::deserialize(&bytes)?;
            return Ok(Some(PageId(m.root_page_id)));
        }
        if bytes.starts_with(META_MAGIC) {
            if bytes.len() < 18 {
                return Err(BTreeError::Corrupt("truncated btree metadata".into()));
            }
            let ver = u16::from_le_bytes([bytes[8], bytes[9]]);
            if ver != META_VERSION {
                return Err(BTreeError::Corrupt(format!(
                    "unsupported btree metadata version {ver}"
                )));
            }
            let crc = u32::from_le_bytes(
                bytes[10..14]
                    .try_into()
                    .map_err(|_| BTreeError::Corrupt("bad metadata crc".into()))?,
            );
            let len = u32::from_le_bytes(
                bytes[14..18]
                    .try_into()
                    .map_err(|_| BTreeError::Corrupt("bad metadata length".into()))?,
            ) as usize;
            if len > MAX_META_BYTES || bytes.len() != 18 + len {
                return Err(BTreeError::Corrupt("invalid btree metadata length".into()));
            }
            let payload = &bytes[18..];
            let mut h = Hasher::new();
            h.update(payload);
            if h.finalize() != crc {
                return Err(BTreeError::Corrupt(
                    "btree metadata checksum mismatch".into(),
                ));
            }
            let m: BTreeMeta = bincode::deserialize(payload)?;
            Ok(Some(PageId(m.root_page_id)))
        } else {
            Err(BTreeError::Corrupt("unknown B+Tree metadata format".into()))
        }
    }
    /// Atomically publishes root metadata using temp fsync, rename and directory fsync.
    /// Implements the `save` operation used by this subsystem.
    pub fn save(&self, root: PageId) -> Result<(), BTreeError> {
        let payload = bincode::serialize(&BTreeMeta {
            root_page_id: root.0,
        })?;
        let mut h = Hasher::new();
        h.update(&payload);
        let crc = h.finalize();
        let len = u32::try_from(payload.len())
            .map_err(|_| BTreeError::Corrupt("metadata payload too large".into()))?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = File::create(&tmp)?;
            f.write_all(META_MAGIC)?;
            f.write_all(&META_VERSION.to_le_bytes())?;
            f.write_all(&crc.to_le_bytes())?;
            f.write_all(&len.to_le_bytes())?;
            f.write_all(&payload)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        if let Some(parent) = self.path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}
