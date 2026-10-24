//! Page-based B+Trees.
//!
//! * [`BPlusTree`]: one generic tree over fixed-width keys mapping to heap locations, used for
//!   both the primary (current-state) index and the temporal (version) index. Keys implement
//!   [`TreeKey`], which fixes their encoded width and node fanout; everything else — routing,
//!   splits, range scans, floor lookups, batched lookups, deletes and journaled persistence — is
//!   shared. The on-disk node layout is identical to the two specialised trees of Milestone
//!   2.0.2, so existing index files remain readable.
//! * [`ByteTree`] (since 2.3.0): variable-length byte keys and byte values with byte-sized
//!   nodes, used by secondary indexes over order-preserving composite keys.
pub mod byte_tree;
pub mod codec;
pub mod error;
pub mod key;
pub mod meta;
pub mod node;
pub mod tree;

pub use byte_tree::{ByteTree, TreeShape, MAX_KEY_BYTES, MAX_VALUE_BYTES};
pub use error::BTreeError;
pub use key::TreeKey;
pub use tree::BPlusTree;

/// Primary index: `RowId -> RowLocation` of the current version.
pub type BTree = BPlusTree<adb_core::RowId>;
/// Temporal index: `(RowId, begin_ts) -> RowLocation` of a historical version.
pub type VersionBTree = BPlusTree<adb_core::VersionKey>;
