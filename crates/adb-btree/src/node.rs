//! Node module for the adb-btree crate.
//!
use adb_core::{PageId, RowId, RowLocation};

/// Defines the `MAX_LEAF_ENTRIES` constant used by this subsystem.
pub const MAX_LEAF_ENTRIES: usize = 96;
/// Defines the `MAX_INTERNAL_KEYS` constant used by this subsystem.
pub const MAX_INTERNAL_KEYS: usize = 96;

/// Enumerates the supported `Node` variants used by this subsystem.
#[derive(Debug, Clone)]
pub enum Node {
    Leaf(LeafNode),
    Internal(InternalNode),
}

/// Represents `LeafNode` state used by the src subsystem.
#[derive(Debug, Clone)]
pub struct LeafNode {
    pub keys: Vec<RowId>,
    pub values: Vec<RowLocation>,
    pub next: Option<PageId>,
}

/// Represents `InternalNode` state used by the src subsystem.
#[derive(Debug, Clone)]
pub struct InternalNode {
    pub keys: Vec<RowId>,
    pub children: Vec<PageId>,
}

/// Implements behavior for `LeafNode`.
impl LeafNode {
    /// Implements the `empty` operation used by this subsystem.
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            values: Vec::new(),
            next: None,
        }
    }
}
