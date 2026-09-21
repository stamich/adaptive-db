//! B+Tree node codec with bounded, panic-free structural validation.

use adb_core::{PageId, RowId, RowLocation};
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE, PAGE_SIZE};

use crate::{
    error::BTreeError,
    node::{InternalNode, LeafNode, Node, MAX_INTERNAL_KEYS, MAX_LEAF_ENTRIES},
};

/// Sentinel persisted when a leaf has no right sibling.
const NONE_PAGE: u64 = u64::MAX;
/// Number of bytes available to a B+Tree node after the common page header.
const PAYLOAD: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
/// Bytes reserved before the first leaf key/value pair.
const LEAF_HEADER: usize = 16;
/// Encoded bytes occupied by one `(RowId, RowLocation)` leaf entry.
const LEAF_ENTRY: usize = 26;
/// Bytes reserved before the first internal child pointer.
const INTERNAL_HEADER: usize = 8;

/// Decodes one current-state B+Tree node after validating count, bounds, shape, and key order.
pub fn decode_node(page: &Page) -> Result<Node, BTreeError> {
    let data = &page.bytes()[PAGE_HEADER_SIZE..];
    match page
        .kind()
        .map_err(|error| BTreeError::Corrupt(error.to_string()))?
    {
        PageKind::BTreeLeaf => {
            let count = read_u16(data, 0)? as usize;
            if count > MAX_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt("leaf count exceeds maximum".into()));
            }
            if LEAF_HEADER + count * LEAF_ENTRY > PAYLOAD {
                return Err(BTreeError::Corrupt("leaf payload exceeds page".into()));
            }

            let next = read_u64(data, 8)?;
            let mut offset = LEAF_HEADER;
            let mut keys = Vec::with_capacity(count);
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                keys.push(RowId(read_u128(data, offset)?));
                offset += 16;
                values.push(RowLocation {
                    page_id: PageId(read_u64(data, offset)?),
                    slot_id: read_u16(data, offset + 8)?,
                });
                offset += 10;
            }
            ensure_sorted(&keys, "leaf")?;
            Ok(Node::Leaf(LeafNode {
                keys,
                values,
                next: (next != NONE_PAGE).then_some(PageId(next)),
            }))
        }
        PageKind::BTreeInternal => {
            let count = read_u16(data, 0)? as usize;
            if count > MAX_INTERNAL_KEYS {
                return Err(BTreeError::Corrupt("internal count exceeds maximum".into()));
            }
            let child_count = count + 1;
            if INTERNAL_HEADER + child_count * 8 + count * 16 > PAYLOAD {
                return Err(BTreeError::Corrupt("internal payload exceeds page".into()));
            }

            let mut offset = INTERNAL_HEADER;
            let mut children = Vec::with_capacity(child_count);
            for _ in 0..child_count {
                children.push(PageId(read_u64(data, offset)?));
                offset += 8;
            }
            let mut keys = Vec::with_capacity(count);
            for _ in 0..count {
                keys.push(RowId(read_u128(data, offset)?));
                offset += 16;
            }
            ensure_sorted(&keys, "internal")?;
            Ok(Node::Internal(InternalNode { keys, children }))
        }
        other => Err(BTreeError::Corrupt(format!(
            "unexpected page kind {other:?}"
        ))),
    }
}

/// Encodes one validated current-state B+Tree node into a page payload.
pub fn encode_node(page: &mut Page, node: &Node) -> Result<(), BTreeError> {
    validate_node(node)?;
    page.set_kind(match node {
        Node::Leaf(_) => PageKind::BTreeLeaf,
        Node::Internal(_) => PageKind::BTreeInternal,
    });

    let data = &mut page.bytes_mut()[PAGE_HEADER_SIZE..];
    data.fill(0);
    match node {
        Node::Leaf(leaf) => {
            write_u16(data, 0, leaf.keys.len() as u16)?;
            write_u64(data, 8, leaf.next.map(|page| page.0).unwrap_or(NONE_PAGE))?;
            let mut offset = LEAF_HEADER;
            for (key, value) in leaf.keys.iter().zip(&leaf.values) {
                write_u128(data, offset, key.0)?;
                offset += 16;
                write_u64(data, offset, value.page_id.0)?;
                write_u16(data, offset + 8, value.slot_id)?;
                offset += 10;
            }
        }
        Node::Internal(internal) => {
            write_u16(data, 0, internal.keys.len() as u16)?;
            let mut offset = INTERNAL_HEADER;
            for child in &internal.children {
                write_u64(data, offset, child.0)?;
                offset += 8;
            }
            for key in &internal.keys {
                write_u128(data, offset, key.0)?;
                offset += 16;
            }
        }
    }
    Ok(())
}

/// Validates node vector shape, maximum fanout, and strict key ordering before persistence.
fn validate_node(node: &Node) -> Result<(), BTreeError> {
    match node {
        Node::Leaf(leaf) => {
            if leaf.keys.len() != leaf.values.len() || leaf.keys.len() > MAX_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt("invalid leaf shape".into()));
            }
            ensure_sorted(&leaf.keys, "leaf")
        }
        Node::Internal(internal) => {
            if internal.keys.len() > MAX_INTERNAL_KEYS
                || internal.children.len() != internal.keys.len() + 1
            {
                return Err(BTreeError::Corrupt("invalid internal shape".into()));
            }
            ensure_sorted(&internal.keys, "internal")
        }
    }
}

/// Requires strictly increasing keys so binary search and routing remain valid.
fn ensure_sorted(keys: &[RowId], kind: &str) -> Result<(), BTreeError> {
    if keys.windows(2).any(|window| window[0] >= window[1]) {
        Err(BTreeError::Corrupt(format!("{kind} keys not sorted")))
    } else {
        Ok(())
    }
}

/// Reads a little-endian `u16` after validating the requested payload range.
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, BTreeError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| BTreeError::Corrupt("u16 read outside payload".into()))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Reads a little-endian `u64` after validating the requested payload range.
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, BTreeError> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| BTreeError::Corrupt("u64 read outside payload".into()))?;
    let mut array = [0; 8];
    array.copy_from_slice(slice);
    Ok(u64::from_le_bytes(array))
}

/// Reads a little-endian `u128` after validating the requested payload range.
fn read_u128(bytes: &[u8], offset: usize) -> Result<u128, BTreeError> {
    let slice = bytes
        .get(offset..offset + 16)
        .ok_or_else(|| BTreeError::Corrupt("u128 read outside payload".into()))?;
    let mut array = [0; 16];
    array.copy_from_slice(slice);
    Ok(u128::from_le_bytes(array))
}

/// Writes a little-endian `u16` after validating the requested payload range.
fn write_u16(bytes: &mut [u8], offset: usize, value: u16) -> Result<(), BTreeError> {
    bytes
        .get_mut(offset..offset + 2)
        .ok_or_else(|| BTreeError::Corrupt("u16 write outside payload".into()))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Writes a little-endian `u64` after validating the requested payload range.
fn write_u64(bytes: &mut [u8], offset: usize, value: u64) -> Result<(), BTreeError> {
    bytes
        .get_mut(offset..offset + 8)
        .ok_or_else(|| BTreeError::Corrupt("u64 write outside payload".into()))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Writes a little-endian `u128` after validating the requested payload range.
fn write_u128(bytes: &mut [u8], offset: usize, value: u128) -> Result<(), BTreeError> {
    bytes
        .get_mut(offset..offset + 16)
        .ok_or_else(|| BTreeError::Corrupt("u128 write outside payload".into()))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}
