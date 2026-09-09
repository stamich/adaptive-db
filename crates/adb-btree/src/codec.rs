//! B+Tree node codec with bounds, shape and ordering validation.

use adb_core::{PageId, RowId, RowLocation};
use adb_page::{PAGE_HEADER_SIZE, PAGE_SIZE, Page, PageKind};

use crate::{
    error::BTreeError,
    node::{InternalNode, LeafNode, MAX_INTERNAL_KEYS, MAX_LEAF_ENTRIES, Node},
};

/// Documents `NONE_PAGE` and its role in this hardened milestone.
const NONE_PAGE: u64 = u64::MAX;
/// Documents `PAYLOAD_LEN` and its role in this hardened milestone.
const PAYLOAD_LEN: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
/// Documents `LEAF_HEADER` and its role in this hardened milestone.
const LEAF_HEADER: usize = 16;
/// Documents `LEAF_ENTRY` and its role in this hardened milestone.
const LEAF_ENTRY: usize = 26;
/// Documents `INTERNAL_HEADER` and its role in this hardened milestone.
const INTERNAL_HEADER: usize = 8;

/// Decodes one B+Tree node without trusting persisted counts or slice ranges.
pub fn decode_node(page: &Page) -> Result<Node, BTreeError> {
    let data = &page.bytes()[PAGE_HEADER_SIZE..];
    match page
        .kind()
        .map_err(|e| BTreeError::Corrupt(e.to_string()))?
    {
        PageKind::BTreeLeaf => {
            let count = read_u16(data, 0)? as usize;
            if count > MAX_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt(format!(
                    "leaf count {count} exceeds maximum"
                )));
            }
            let required = LEAF_HEADER
                .checked_add(
                    count
                        .checked_mul(LEAF_ENTRY)
                        .ok_or_else(|| BTreeError::Corrupt("leaf size overflow".into()))?,
                )
                .ok_or_else(|| BTreeError::Corrupt("leaf size overflow".into()))?;
            if required > PAYLOAD_LEN {
                return Err(BTreeError::Corrupt("leaf payload exceeds page".into()));
            }
            let next_raw = read_u64(data, 8)?;
            let mut off = LEAF_HEADER;
            let mut keys = Vec::with_capacity(count);
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                let key = RowId(read_u128(data, off)?);
                off += 16;
                let page_id = PageId(read_u64(data, off)?);
                off += 8;
                let slot_id = read_u16(data, off)?;
                off += 2;
                keys.push(key);
                values.push(RowLocation { page_id, slot_id });
            }
            ensure_strictly_sorted(&keys, "leaf")?;
            Ok(Node::Leaf(LeafNode {
                keys,
                values,
                next: (next_raw != NONE_PAGE).then_some(PageId(next_raw)),
            }))
        }
        PageKind::BTreeInternal => {
            let key_count = read_u16(data, 0)? as usize;
            if key_count > MAX_INTERNAL_KEYS {
                return Err(BTreeError::Corrupt(format!(
                    "internal key count {key_count} exceeds maximum"
                )));
            }
            let child_count = key_count
                .checked_add(1)
                .ok_or_else(|| BTreeError::Corrupt("child count overflow".into()))?;
            let required = INTERNAL_HEADER
                .checked_add(
                    child_count
                        .checked_mul(8)
                        .ok_or_else(|| BTreeError::Corrupt("internal size overflow".into()))?,
                )
                .and_then(|v| v.checked_add(key_count.checked_mul(16)?))
                .ok_or_else(|| BTreeError::Corrupt("internal size overflow".into()))?;
            if required > PAYLOAD_LEN {
                return Err(BTreeError::Corrupt("internal payload exceeds page".into()));
            }
            let mut off = INTERNAL_HEADER;
            let mut children = Vec::with_capacity(child_count);
            for _ in 0..child_count {
                children.push(PageId(read_u64(data, off)?));
                off += 8;
            }
            let mut keys = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                keys.push(RowId(read_u128(data, off)?));
                off += 16;
            }
            ensure_strictly_sorted(&keys, "internal")?;
            Ok(Node::Internal(InternalNode { keys, children }))
        }
        other => Err(BTreeError::Corrupt(format!(
            "unexpected page kind {other:?}"
        ))),
    }
}

/// Encodes one structurally valid B+Tree node into a page payload.
pub fn encode_node(page: &mut Page, node: &Node) -> Result<(), BTreeError> {
    validate_node(node)?;
    match node {
        Node::Leaf(_) => page.set_kind(PageKind::BTreeLeaf),
        Node::Internal(_) => page.set_kind(PageKind::BTreeInternal),
    }
    let payload = &mut page.bytes_mut()[PAGE_HEADER_SIZE..];
    payload.fill(0);
    match node {
        Node::Leaf(leaf) => {
            write_u16(payload, 0, leaf.keys.len() as u16)?;
            write_u64(payload, 8, leaf.next.map(|p| p.0).unwrap_or(NONE_PAGE))?;
            let mut off = LEAF_HEADER;
            for (key, value) in leaf.keys.iter().zip(&leaf.values) {
                write_u128(payload, off, key.0)?;
                off += 16;
                write_u64(payload, off, value.page_id.0)?;
                off += 8;
                write_u16(payload, off, value.slot_id)?;
                off += 2;
            }
        }
        Node::Internal(internal) => {
            write_u16(payload, 0, internal.keys.len() as u16)?;
            let mut off = INTERNAL_HEADER;
            for child in &internal.children {
                write_u64(payload, off, child.0)?;
                off += 8;
            }
            for key in &internal.keys {
                write_u128(payload, off, key.0)?;
                off += 16;
            }
        }
    }
    Ok(())
}

/// Validates vector lengths, maximum fanout and strict key order before persistence.
fn validate_node(node: &Node) -> Result<(), BTreeError> {
    match node {
        Node::Leaf(leaf) => {
            if leaf.keys.len() != leaf.values.len() {
                return Err(BTreeError::Corrupt("leaf key/value length mismatch".into()));
            }
            if leaf.keys.len() > MAX_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt("leaf exceeds maximum entries".into()));
            }
            ensure_strictly_sorted(&leaf.keys, "leaf")
        }
        Node::Internal(internal) => {
            if internal.keys.len() > MAX_INTERNAL_KEYS {
                return Err(BTreeError::Corrupt("internal exceeds maximum keys".into()));
            }
            if internal.children.len() != internal.keys.len() + 1 {
                return Err(BTreeError::Corrupt(
                    "internal child/key shape mismatch".into(),
                ));
            }
            ensure_strictly_sorted(&internal.keys, "internal")
        }
    }
}

/// Ensures keys are strictly increasing so binary search/routing semantics remain valid.
fn ensure_strictly_sorted(keys: &[RowId], kind: &str) -> Result<(), BTreeError> {
    if keys.windows(2).any(|w| w[0] >= w[1]) {
        return Err(BTreeError::Corrupt(format!(
            "{kind} keys are not strictly sorted"
        )));
    }
    Ok(())
}

/// Reads a little-endian u16 after checking the requested slice.
fn read_u16(b: &[u8], o: usize) -> Result<u16, BTreeError> {
    let s = b
        .get(o..o + 2)
        .ok_or_else(|| BTreeError::Corrupt("u16 read outside node payload".into()))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}
/// Reads a little-endian u64 after checking the requested slice.
fn read_u64(b: &[u8], o: usize) -> Result<u64, BTreeError> {
    let s = b
        .get(o..o + 8)
        .ok_or_else(|| BTreeError::Corrupt("u64 read outside node payload".into()))?;
    Ok(u64::from_le_bytes(s.try_into().map_err(|_| {
        BTreeError::Corrupt("invalid u64 bytes".into())
    })?))
}
/// Reads a little-endian u128 after checking the requested slice.
fn read_u128(b: &[u8], o: usize) -> Result<u128, BTreeError> {
    let s = b
        .get(o..o + 16)
        .ok_or_else(|| BTreeError::Corrupt("u128 read outside node payload".into()))?;
    Ok(u128::from_le_bytes(s.try_into().map_err(|_| {
        BTreeError::Corrupt("invalid u128 bytes".into())
    })?))
}
/// Writes a little-endian u16 after checking the requested slice.
fn write_u16(b: &mut [u8], o: usize, v: u16) -> Result<(), BTreeError> {
    b.get_mut(o..o + 2)
        .ok_or_else(|| BTreeError::Corrupt("u16 write outside node payload".into()))?
        .copy_from_slice(&v.to_le_bytes());
    Ok(())
}
/// Writes a little-endian u64 after checking the requested slice.
fn write_u64(b: &mut [u8], o: usize, v: u64) -> Result<(), BTreeError> {
    b.get_mut(o..o + 8)
        .ok_or_else(|| BTreeError::Corrupt("u64 write outside node payload".into()))?
        .copy_from_slice(&v.to_le_bytes());
    Ok(())
}
/// Writes a little-endian u128 after checking the requested slice.
fn write_u128(b: &mut [u8], o: usize, v: u128) -> Result<(), BTreeError> {
    b.get_mut(o..o + 16)
        .ok_or_else(|| BTreeError::Corrupt("u128 write outside node payload".into()))?
        .copy_from_slice(&v.to_le_bytes());
    Ok(())
}
