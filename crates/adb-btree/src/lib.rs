//! Lib module for the adb-btree crate.
//!
pub mod codec;
pub mod error;
pub mod meta;
pub mod node;
pub mod tree;
mod version_tree;
mod version_node;
mod version_codec;

pub use error::BTreeError;
pub use tree::BTree;
