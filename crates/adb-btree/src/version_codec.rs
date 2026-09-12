//! Version B+Tree codec with bounded, panic-free structural validation.
use crate::{
    error::BTreeError,
    version_node::{
        VersionInternalNode, VersionLeafNode, VersionNode, MAX_VERSION_INTERNAL_KEYS,
        MAX_VERSION_LEAF_ENTRIES,
    },
};
use adb_core::{CommitTs, PageId, RowId, RowLocation, VersionKey};
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE, PAGE_SIZE};
/// Defines the `NONE_PAGE` constant used by this subsystem.
const NONE_PAGE: u64 = u64::MAX;
const PAYLOAD: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
const LEAF_HEADER: usize = 16;
const LEAF_ENTRY: usize = 34;
const INTERNAL_HEADER: usize = 8;
const KEY_SIZE: usize = 24;
/// Decodes one temporal-index node after validating count, bounds, shape and VersionKey order.
pub fn decode_version_node(page: &Page) -> Result<VersionNode, BTreeError> {
    let d = &page.bytes()[PAGE_HEADER_SIZE..];
    match page
        .kind()
        .map_err(|e| BTreeError::Corrupt(e.to_string()))?
    {
        PageKind::BTreeLeaf => {
            let n = r16(d, 0)? as usize;
            if n > MAX_VERSION_LEAF_ENTRIES || LEAF_HEADER + n * LEAF_ENTRY > PAYLOAD {
                return Err(BTreeError::Corrupt(
                    "invalid version leaf count/size".into(),
                ));
            }
            let next = r64(d, 8)?;
            let mut o = 16;
            let mut ks = Vec::with_capacity(n);
            let mut vs = Vec::with_capacity(n);
            for _ in 0..n {
                let k = VersionKey::new(RowId(r128(d, o)?), CommitTs(r64(d, o + 16)?));
                o += 24;
                vs.push(RowLocation {
                    page_id: PageId(r64(d, o)?),
                    slot_id: r16(d, o + 8)?,
                });
                o += 10;
                ks.push(k);
            }
            sorted(&ks, "version leaf")?;
            Ok(VersionNode::Leaf(VersionLeafNode {
                keys: ks,
                values: vs,
                next: (next != NONE_PAGE).then_some(PageId(next)),
            }))
        }
        PageKind::BTreeInternal => {
            let n = r16(d, 0)? as usize;
            if n > MAX_VERSION_INTERNAL_KEYS {
                return Err(BTreeError::Corrupt("invalid version internal count".into()));
            }
            let c = n + 1;
            if INTERNAL_HEADER + c * 8 + n * KEY_SIZE > PAYLOAD {
                return Err(BTreeError::Corrupt(
                    "version internal payload exceeds page".into(),
                ));
            }
            let mut o = 8;
            let mut cs = Vec::with_capacity(c);
            for _ in 0..c {
                cs.push(PageId(r64(d, o)?));
                o += 8;
            }
            let mut ks = Vec::with_capacity(n);
            for _ in 0..n {
                ks.push(VersionKey::new(
                    RowId(r128(d, o)?),
                    CommitTs(r64(d, o + 16)?),
                ));
                o += 24;
            }
            sorted(&ks, "version internal")?;
            Ok(VersionNode::Internal(VersionInternalNode {
                keys: ks,
                children: cs,
            }))
        }
        other => Err(BTreeError::Corrupt(format!(
            "unexpected page kind {other:?}"
        ))),
    }
}
/// Encodes one structurally valid temporal-index node.
pub fn encode_version_node(page: &mut Page, node: &VersionNode) -> Result<(), BTreeError> {
    validate(node)?;
    page.set_kind(match node {
        VersionNode::Leaf(_) => PageKind::BTreeLeaf,
        VersionNode::Internal(_) => PageKind::BTreeInternal,
    });
    let d = &mut page.bytes_mut()[PAGE_HEADER_SIZE..];
    d.fill(0);
    match node {
        VersionNode::Leaf(l) => {
            w16(d, 0, l.keys.len() as u16)?;
            w64(d, 8, l.next.map(|p| p.0).unwrap_or(NONE_PAGE))?;
            let mut o = 16;
            for (k, v) in l.keys.iter().zip(&l.values) {
                w128(d, o, k.row_id.0)?;
                w64(d, o + 16, k.begin_ts.0)?;
                o += 24;
                w64(d, o, v.page_id.0)?;
                w16(d, o + 8, v.slot_id)?;
                o += 10;
            }
        }
        VersionNode::Internal(i) => {
            w16(d, 0, i.keys.len() as u16)?;
            let mut o = 8;
            for c in &i.children {
                w64(d, o, c.0)?;
                o += 8;
            }
            for k in &i.keys {
                w128(d, o, k.row_id.0)?;
                w64(d, o + 16, k.begin_ts.0)?;
                o += 24;
            }
        }
    }
    Ok(())
}
/// Validates temporal-node vector shape and key ordering.
fn validate(n: &VersionNode) -> Result<(), BTreeError> {
    match n {
        VersionNode::Leaf(l) => {
            if l.keys.len() != l.values.len() || l.keys.len() > MAX_VERSION_LEAF_ENTRIES {
                return Err(BTreeError::Corrupt("invalid version leaf shape".into()));
            }
            sorted(&l.keys, "version leaf")
        }
        VersionNode::Internal(i) => {
            if i.keys.len() > MAX_VERSION_INTERNAL_KEYS || i.children.len() != i.keys.len() + 1 {
                return Err(BTreeError::Corrupt("invalid version internal shape".into()));
            }
            sorted(&i.keys, "version internal")
        }
    }
}
/// Requires strictly increasing temporal keys.
fn sorted(k: &[VersionKey], kind: &str) -> Result<(), BTreeError> {
    if k.windows(2).any(|w| w[0] >= w[1]) {
        Err(BTreeError::Corrupt(format!("{kind} keys not sorted")))
    } else {
        Ok(())
    }
}
/// Reads u16 safely.
fn r16(b: &[u8], o: usize) -> Result<u16, BTreeError> {
    let s = b
        .get(o..o + 2)
        .ok_or_else(|| BTreeError::Corrupt("u16 read outside payload".into()))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}
/// Reads u64 safely.
fn r64(b: &[u8], o: usize) -> Result<u64, BTreeError> {
    let s = b
        .get(o..o + 8)
        .ok_or_else(|| BTreeError::Corrupt("u64 read outside payload".into()))?;
    let mut a = [0; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}
/// Reads u128 safely.
fn r128(b: &[u8], o: usize) -> Result<u128, BTreeError> {
    let s = b
        .get(o..o + 16)
        .ok_or_else(|| BTreeError::Corrupt("u128 read outside payload".into()))?;
    let mut a = [0; 16];
    a.copy_from_slice(s);
    Ok(u128::from_le_bytes(a))
}
/// Writes u16 safely.
fn w16(b: &mut [u8], o: usize, v: u16) -> Result<(), BTreeError> {
    b.get_mut(o..o + 2)
        .ok_or_else(|| BTreeError::Corrupt("u16 write outside payload".into()))?
        .copy_from_slice(&v.to_le_bytes());
    Ok(())
}
/// Writes u64 safely.
fn w64(b: &mut [u8], o: usize, v: u64) -> Result<(), BTreeError> {
    b.get_mut(o..o + 8)
        .ok_or_else(|| BTreeError::Corrupt("u64 write outside payload".into()))?
        .copy_from_slice(&v.to_le_bytes());
    Ok(())
}
/// Writes u128 safely.
fn w128(b: &mut [u8], o: usize, v: u128) -> Result<(), BTreeError> {
    b.get_mut(o..o + 16)
        .ok_or_else(|| BTreeError::Corrupt("u128 write outside payload".into()))?
        .copy_from_slice(&v.to_le_bytes());
    Ok(())
}
