//! Fixed-width key encodings supported by [`crate::BPlusTree`].
use std::fmt::Debug;

use adb_core::{CommitTs, RowId, VersionKey};

/// A totally ordered key with a fixed little-endian encoding.
pub trait TreeKey: Copy + Ord + Debug + Send + Sync + 'static {
    /// Encoded width in bytes.
    const ENCODED_LEN: usize;
    /// Maximum entries in a leaf node (must fit one page).
    const MAX_LEAF_ENTRIES: usize;
    /// Maximum separator keys in an internal node (must fit one page).
    const MAX_INTERNAL_KEYS: usize;
    /// Writes exactly [`Self::ENCODED_LEN`] bytes into `out`.
    fn encode(&self, out: &mut [u8]);
    /// Reads a key from exactly [`Self::ENCODED_LEN`] bytes.
    fn decode(bytes: &[u8]) -> Self;
}

impl TreeKey for RowId {
    const ENCODED_LEN: usize = 16;
    const MAX_LEAF_ENTRIES: usize = 96;
    const MAX_INTERNAL_KEYS: usize = 96;

    fn encode(&self, out: &mut [u8]) {
        out.copy_from_slice(&self.0.to_le_bytes());
    }

    fn decode(bytes: &[u8]) -> Self {
        let mut array = [0u8; 16];
        array.copy_from_slice(bytes);
        RowId(u128::from_le_bytes(array))
    }
}

impl TreeKey for VersionKey {
    const ENCODED_LEN: usize = 24;
    const MAX_LEAF_ENTRIES: usize = 72;
    const MAX_INTERNAL_KEYS: usize = 72;

    fn encode(&self, out: &mut [u8]) {
        self.row_id.encode(&mut out[..16]);
        out[16..24].copy_from_slice(&self.begin_ts.0.to_le_bytes());
    }

    fn decode(bytes: &[u8]) -> Self {
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&bytes[16..24]);
        VersionKey::new(
            RowId::decode(&bytes[..16]),
            CommitTs(u64::from_le_bytes(ts)),
        )
    }
}
