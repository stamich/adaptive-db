//! Page encoding of B+Tree nodes with bounded, panic-free validation.
//!
//! ```text
//! leaf:     u16 count | 6 pad | u64 next (u64::MAX = none) | count x (key | u64 page | u16 slot)
//! internal: u16 count | 6 pad | (count + 1) x u64 child | count x key
//! ```
//! Offsets are relative to the end of the common page header; integers are little-endian.

use adb_core::{PageId, RowLocation};
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE, PAGE_SIZE};

use crate::{
    error::BTreeError,
    key::TreeKey,
    node::{InternalNode, LeafNode, Node},
};

/// Sentinel stored in `next` when a leaf has no right sibling.
const NONE_PAGE: u64 = u64::MAX;
/// Bytes available to a node after the common page header.
const PAYLOAD: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
/// Leaf header: entry count (u16), padding, right-sibling page id (u64).
const LEAF_HEADER: usize = 16;
/// Internal-node header: key count (u16) plus padding.
const INTERNAL_HEADER: usize = 8;
/// Encoded size of a `RowLocation` (page id u64 + slot u16).
const LOCATION_LEN: usize = 10;

/// Encoded size of one leaf entry: key followed by its location.
fn leaf_entry_len<K: TreeKey>() -> usize {
    K::ENCODED_LEN + LOCATION_LEN
}

/// Decodes and validates one node page.
pub fn decode_node<K: TreeKey>(page: &Page) -> Result<Node<K>, BTreeError> {
    let data = &page.bytes()[PAGE_HEADER_SIZE..];
    match page
        .kind()
        .map_err(|e| BTreeError::Corrupt(e.to_string()))?
    {
        PageKind::BTreeLeaf => {
            let count = read_u16(data, 0)? as usize;
            if count > K::MAX_LEAF_ENTRIES || LEAF_HEADER + count * leaf_entry_len::<K>() > PAYLOAD
            {
                return Err(BTreeError::Corrupt("invalid leaf count".into()));
            }
            let next = read_u64(data, 8)?;
            let mut keys = Vec::with_capacity(count);
            let mut values = Vec::with_capacity(count);
            let mut at = LEAF_HEADER;
            for _ in 0..count {
                keys.push(read_key::<K>(data, at)?);
                at += K::ENCODED_LEN;
                values.push(RowLocation {
                    page_id: PageId(read_u64(data, at)?),
                    slot_id: read_u16(data, at + 8)?,
                });
                at += LOCATION_LEN;
            }
            ensure_sorted(&keys)?;
            Ok(Node::Leaf(LeafNode {
                keys,
                values,
                next: (next != NONE_PAGE).then_some(PageId(next)),
            }))
        }
        PageKind::BTreeInternal => {
            let count = read_u16(data, 0)? as usize;
            if count > K::MAX_INTERNAL_KEYS
                || INTERNAL_HEADER + (count + 1) * 8 + count * K::ENCODED_LEN > PAYLOAD
            {
                return Err(BTreeError::Corrupt("invalid internal count".into()));
            }
            let mut at = INTERNAL_HEADER;
            let mut children = Vec::with_capacity(count + 1);
            for _ in 0..=count {
                children.push(PageId(read_u64(data, at)?));
                at += 8;
            }
            let mut keys = Vec::with_capacity(count);
            for _ in 0..count {
                keys.push(read_key::<K>(data, at)?);
                at += K::ENCODED_LEN;
            }
            ensure_sorted(&keys)?;
            Ok(Node::Internal(InternalNode { keys, children }))
        }
        other => Err(BTreeError::Corrupt(format!(
            "unexpected page kind {other:?}"
        ))),
    }
}

/// Validates and encodes one node into `page`.
pub fn encode_node<K: TreeKey>(page: &mut Page, node: &Node<K>) -> Result<(), BTreeError> {
    validate(node)?;
    page.set_kind(match node {
        Node::Leaf(_) => PageKind::BTreeLeaf,
        Node::Internal(_) => PageKind::BTreeInternal,
    });
    let data = &mut page.bytes_mut()[PAGE_HEADER_SIZE..];
    data.fill(0);
    match node {
        Node::Leaf(leaf) => {
            write(data, 0, &(leaf.keys.len() as u16).to_le_bytes())?;
            write(data, 8, &leaf.next.map_or(NONE_PAGE, |p| p.0).to_le_bytes())?;
            let mut at = LEAF_HEADER;
            for (key, value) in leaf.keys.iter().zip(&leaf.values) {
                write_key(data, at, key)?;
                at += K::ENCODED_LEN;
                write(data, at, &value.page_id.0.to_le_bytes())?;
                write(data, at + 8, &value.slot_id.to_le_bytes())?;
                at += LOCATION_LEN;
            }
        }
        Node::Internal(internal) => {
            write(data, 0, &(internal.keys.len() as u16).to_le_bytes())?;
            let mut at = INTERNAL_HEADER;
            for child in &internal.children {
                write(data, at, &child.0.to_le_bytes())?;
                at += 8;
            }
            for key in &internal.keys {
                write_key(data, at, key)?;
                at += K::ENCODED_LEN;
            }
        }
    }
    Ok(())
}

/// Checks parallel vector lengths, fanout limits and key order before encoding.
fn validate<K: TreeKey>(node: &Node<K>) -> Result<(), BTreeError> {
    match node {
        Node::Leaf(leaf) => {
            if leaf.keys.len() != leaf.values.len() || leaf.keys.len() > K::MAX_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt("invalid leaf shape".into()));
            }
            ensure_sorted(&leaf.keys)
        }
        Node::Internal(internal) => {
            if internal.keys.len() > K::MAX_INTERNAL_KEYS
                || internal.children.len() != internal.keys.len() + 1
            {
                return Err(BTreeError::Corrupt("invalid internal shape".into()));
            }
            ensure_sorted(&internal.keys)
        }
    }
}

/// Requires strictly increasing keys, which binary search and routing rely on.
fn ensure_sorted<K: Ord>(keys: &[K]) -> Result<(), BTreeError> {
    if keys.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(BTreeError::Corrupt("node keys not strictly sorted".into()));
    }
    Ok(())
}

/// Bounds-checked sub-slice of a node payload.
fn slice(data: &[u8], at: usize, len: usize) -> Result<&[u8], BTreeError> {
    data.get(at..at + len)
        .ok_or_else(|| BTreeError::Corrupt("read outside node payload".into()))
}

/// Decodes a key at `at`.
fn read_key<K: TreeKey>(data: &[u8], at: usize) -> Result<K, BTreeError> {
    Ok(K::decode(slice(data, at, K::ENCODED_LEN)?))
}

/// Reads a little-endian `u16` at `at`.
fn read_u16(data: &[u8], at: usize) -> Result<u16, BTreeError> {
    let bytes = slice(data, at, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Reads a little-endian `u64` at `at`.
fn read_u64(data: &[u8], at: usize) -> Result<u64, BTreeError> {
    let mut array = [0u8; 8];
    array.copy_from_slice(slice(data, at, 8)?);
    Ok(u64::from_le_bytes(array))
}

/// Bounds-checked copy of `bytes` into the payload at `at`.
fn write(data: &mut [u8], at: usize, bytes: &[u8]) -> Result<(), BTreeError> {
    data.get_mut(at..at + bytes.len())
        .ok_or_else(|| BTreeError::Corrupt("write outside node payload".into()))?
        .copy_from_slice(bytes);
    Ok(())
}

/// Encodes a key at `at`.
fn write_key<K: TreeKey>(data: &mut [u8], at: usize, key: &K) -> Result<(), BTreeError> {
    let target = data
        .get_mut(at..at + K::ENCODED_LEN)
        .ok_or_else(|| BTreeError::Corrupt("write outside node payload".into()))?;
    key.encode(target);
    Ok(())
}
