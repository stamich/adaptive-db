//! Module `version_node` for crate `adb-btree`.
use adb_core::{PageId, RowLocation, VersionKey};

/// Defines the `MAX_VERSION_LEAF_ENTRIES` constant used by this subsystem.
pub const MAX_VERSION_LEAF_ENTRIES: usize = 72;
/// Defines the `MAX_VERSION_INTERNAL_KEYS` constant used by this subsystem.
pub const MAX_VERSION_INTERNAL_KEYS: usize = 72;

/// Enumerates `VersionNode` alternatives used by this subsystem.
#[derive(Debug, Clone)]
pub enum VersionNode {
    Leaf(VersionLeafNode),
    Internal(VersionInternalNode),
}

/// Represents `VersionLeafNode` state used by this subsystem.
#[derive(Debug, Clone)]
pub struct VersionLeafNode {
    pub keys: Vec<VersionKey>,
    pub values: Vec<RowLocation>,
    pub next: Option<PageId>,
}

/// Represents `VersionInternalNode` state used by this subsystem.
#[derive(Debug, Clone)]
pub struct VersionInternalNode {
    pub keys: Vec<VersionKey>,
    pub children: Vec<PageId>,
}

/// Implements behavior for `VersionLeafNode`.
impl VersionLeafNode {
    /// Implements the `empty` operation used by this subsystem.
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            values: Vec::new(),
            next: None,
        }
    }
}
