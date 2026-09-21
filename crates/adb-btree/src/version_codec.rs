//! Version B+Tree codec with bounded, panic-free structural validation.

use adb_core::{CommitTs, PageId, RowId, RowLocation, VersionKey};
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE, PAGE_SIZE};

use crate::{
    error::BTreeError,
    version_node::{
        VersionInternalNode, VersionLeafNode, VersionNode, MAX_VERSION_INTERNAL_KEYS,
        MAX_VERSION_LEAF_ENTRIES,
    },
};

/// Sentinel persisted when a temporal leaf has no right sibling.
const NONE_PAGE: u64 = u64::MAX;
/// Number of payload bytes after the common page header.
const PAYLOAD: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
/// Bytes reserved before temporal leaf entries.
const LEAF_HEADER: usize = 16;
/// Encoded bytes occupied by one `(VersionKey, RowLocation)` entry.
const LEAF_ENTRY: usize = 34;
/// Bytes reserved before temporal internal children.
const INTERNAL_HEADER: usize = 8;
/// Encoded bytes occupied by one temporal `VersionKey`.
const KEY_SIZE: usize = 24;

/// Decodes one temporal-index node after validating count, bounds, shape, and `VersionKey` order.
pub fn decode_version_node(page: &Page) -> Result<VersionNode, BTreeError> {
    let data = &page.bytes()[PAGE_HEADER_SIZE..];
    match page
        .kind()
        .map_err(|error| BTreeError::Corrupt(error.to_string()))?
    {
        PageKind::BTreeLeaf => {
            let count = read_u16(data, 0)? as usize;
            if count > MAX_VERSION_LEAF_ENTRIES || LEAF_HEADER + count * LEAF_ENTRY > PAYLOAD {
                return Err(BTreeError::Corrupt(
                    "invalid version leaf count/size".into(),
                ));
            }

            let next = read_u64(data, 8)?;
            let mut offset = LEAF_HEADER;
            let mut keys = Vec::with_capacity(count);
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                let key = VersionKey::new(
                    RowId(read_u128(data, offset)?),
                    CommitTs(read_u64(data, offset + 16)?),
                );
                offset += KEY_SIZE;
                values.push(RowLocation {
                    page_id: PageId(read_u64(data, offset)?),
                    slot_id: read_u16(data, offset + 8)?,
                });
                offset += 10;
                keys.push(key);
            }
            ensure_sorted(&keys, "version leaf")?;
            Ok(VersionNode::Leaf(VersionLeafNode {
                keys,
                values,
                next: (next != NONE_PAGE).then_some(PageId(next)),
            }))
        }
        PageKind::BTreeInternal => {
            let count = read_u16(data, 0)? as usize;
            if count > MAX_VERSION_INTERNAL_KEYS {
                return Err(BTreeError::Corrupt("invalid version internal count".into()));
            }
            let child_count = count + 1;
            if INTERNAL_HEADER + child_count * 8 + count * KEY_SIZE > PAYLOAD {
                return Err(BTreeError::Corrupt(
                    "version internal payload exceeds page".into(),
                ));
            }

            let mut offset = INTERNAL_HEADER;
            let mut children = Vec::with_capacity(child_count);
            for _ in 0..child_count {
                children.push(PageId(read_u64(data, offset)?));
                offset += 8;
            }
            let mut keys = Vec::with_capacity(count);
            for _ in 0..count {
                keys.push(VersionKey::new(
                    RowId(read_u128(data, offset)?),
                    CommitTs(read_u64(data, offset + 16)?),
                ));
                offset += KEY_SIZE;
            }
            ensure_sorted(&keys, "version internal")?;
            Ok(VersionNode::Internal(VersionInternalNode {
                keys,
                children,
            }))
        }
        other => Err(BTreeError::Corrupt(format!(
            "unexpected page kind {other:?}"
        ))),
    }
}

/// Encodes one structurally valid temporal-index node into a page payload.
pub fn encode_version_node(page: &mut Page, node: &VersionNode) -> Result<(), BTreeError> {
    validate_node(node)?;
    page.set_kind(match node {
        VersionNode::Leaf(_) => PageKind::BTreeLeaf,
        VersionNode::Internal(_) => PageKind::BTreeInternal,
    });

    let data = &mut page.bytes_mut()[PAGE_HEADER_SIZE..];
    data.fill(0);
    match node {
        VersionNode::Leaf(leaf) => {
            write_u16(data, 0, leaf.keys.len() as u16)?;
            write_u64(data, 8, leaf.next.map(|page| page.0).unwrap_or(NONE_PAGE))?;
            let mut offset = LEAF_HEADER;
            for (key, value) in leaf.keys.iter().zip(&leaf.values) {
                write_u128(data, offset, key.row_id.0)?;
                write_u64(data, offset + 16, key.begin_ts.0)?;
                offset += KEY_SIZE;
                write_u64(data, offset, value.page_id.0)?;
                write_u16(data, offset + 8, value.slot_id)?;
                offset += 10;
            }
        }
        VersionNode::Internal(internal) => {
            write_u16(data, 0, internal.keys.len() as u16)?;
            let mut offset = INTERNAL_HEADER;
            for child in &internal.children {
                write_u64(data, offset, child.0)?;
                offset += 8;
            }
            for key in &internal.keys {
                write_u128(data, offset, key.row_id.0)?;
                write_u64(data, offset + 16, key.begin_ts.0)?;
                offset += KEY_SIZE;
            }
        }
    }
    Ok(())
}

/// Validates temporal-node vector shape, maximum fanout, and strict key ordering.
fn validate_node(node: &VersionNode) -> Result<(), BTreeError> {
    match node {
        VersionNode::Leaf(leaf) => {
            if leaf.keys.len() != leaf.values.len() || leaf.keys.len() > MAX_VERSION_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt("invalid version leaf shape".into()));
            }
            ensure_sorted(&leaf.keys, "version leaf")
        }
        VersionNode::Internal(internal) => {
            if internal.keys.len() > MAX_VERSION_INTERNAL_KEYS
                || internal.children.len() != internal.keys.len() + 1
            {
                return Err(BTreeError::Corrupt("invalid version internal shape".into()));
            }
            ensure_sorted(&internal.keys, "version internal")
        }
    }
}

/// Requires strictly increasing temporal keys.
fn ensure_sorted(keys: &[VersionKey], kind: &str) -> Result<(), BTreeError> {
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
