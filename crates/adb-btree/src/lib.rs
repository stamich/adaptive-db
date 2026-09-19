//! Module `lib` for crate `adb-btree`.
pub mod codec;
pub mod error;
pub mod meta;
pub mod node;
pub mod tree;
pub mod version_codec;
pub mod version_node;
pub mod version_tree;

pub use error::BTreeError;
pub use tree::BTree;
pub use version_tree::VersionBTree;
