//! In-memory representation of B+Tree nodes.
use adb_core::{PageId, RowLocation};

/// A decoded node.
#[derive(Debug, Clone)]
pub enum Node<K> {
    /// Leaf with ordered entries and a right-sibling link.
    Leaf(LeafNode<K>),
    /// Internal node with `keys.len() + 1` children.
    Internal(InternalNode<K>),
}

/// Leaf node: `keys[i] -> values[i]`, strictly increasing keys.
#[derive(Debug, Clone)]
pub struct LeafNode<K> {
    /// Ordered keys.
    pub keys: Vec<K>,
    /// Heap locations, parallel to `keys`.
    pub values: Vec<RowLocation>,
    /// Next leaf in key order.
    pub next: Option<PageId>,
}

/// Internal node: child `i` holds keys `< keys[i]`, child `i + 1` holds keys `>= keys[i]`.
#[derive(Debug, Clone)]
pub struct InternalNode<K> {
    /// Separator keys.
    pub keys: Vec<K>,
    /// Child pages.
    pub children: Vec<PageId>,
}

impl<K> LeafNode<K> {
    /// An empty leaf with no sibling.
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            values: Vec::new(),
            next: None,
        }
    }
}

impl<K: Ord> InternalNode<K> {
    /// Index of the child that may contain `key`.
    pub fn route(&self, key: &K) -> usize {
        self.keys.partition_point(|separator| separator <= key)
    }
}
