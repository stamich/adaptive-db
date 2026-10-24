//! Index entries: MVCC-versioned `(key, row id, begin) -> (end, covered values)` pairs.
//!
//! An entry says "from commit `begin_ts` until commit `end_ts` (exclusive), the row `row_id` had
//! the key `key` and the covered values `included`". An update that changes a row's key or
//! covered values ends the old entry and starts a new one, so a snapshot at any retained
//! timestamp sees exactly the entries of the rows visible to it ([`IndexEntry::visible_at`]).
//!
//! Stored in a byte tree as:
//!
//! ```text
//! tree key:   encode_key(key columns) | row id (16 bytes, big-endian) | begin_ts (8 bytes, big-endian)
//! tree value: end_ts (8 bytes, big-endian) | encode_key(covered columns, ascending)
//! ```
//!
//! The key columns come first, so the tree order is the index order and a range of key values is
//! a range of tree keys; the row id makes entries of equal keys unique (and orders them by
//! primary key), the begin timestamp keeps successive versions of one row apart.
use adb_core::{decode_key, encode_key, CommitTs, Direction, RowId, Value};

use crate::{
    definition::{IndexDefinition, MAX_INCLUDE_BYTES, MAX_INDEX_KEY_BYTES},
    IndexError,
};

/// `end_ts` of an entry that is still current.
pub const OPEN: CommitTs = CommitTs(u64::MAX);
/// Bytes after the encoded key columns: row id and begin timestamp.
pub const SUFFIX_BYTES: usize = 16 + 8;

/// One decoded entry.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// Values of the key columns.
    pub key: Vec<Value>,
    /// Row the entry points at.
    pub row_id: RowId,
    /// First commit at which the entry holds.
    pub begin_ts: CommitTs,
    /// Commit from which it no longer holds ([`OPEN`] while current).
    pub end_ts: CommitTs,
    /// Values of the covered columns.
    pub included: Vec<Value>,
}

impl IndexEntry {
    /// Whether a snapshot at `ts` sees the entry: `begin_ts <= ts < end_ts`.
    pub fn visible_at(&self, ts: CommitTs) -> bool {
        self.begin_ts <= ts && ts < self.end_ts
    }
}

/// Encoded key columns of `values` under `definition`, checked against [`MAX_INDEX_KEY_BYTES`].
pub fn encode_index_key(
    definition: &IndexDefinition,
    values: &[Value],
) -> Result<Vec<u8>, IndexError> {
    let key = encode_key(
        values
            .iter()
            .zip(definition.columns.iter().map(|c| c.direction)),
    );
    if key.len() > MAX_INDEX_KEY_BYTES {
        return Err(IndexError::KeyTooLarge {
            index: definition.name.clone(),
            bytes: key.len(),
            limit: MAX_INDEX_KEY_BYTES,
        });
    }
    Ok(key)
}

/// Encoded covered values, checked against [`MAX_INCLUDE_BYTES`].
pub fn encode_included(
    definition: &IndexDefinition,
    values: &[Value],
) -> Result<Vec<u8>, IndexError> {
    let payload = encode_key(values.iter().map(|value| (value, Direction::Asc)));
    if payload.len() > MAX_INCLUDE_BYTES {
        return Err(IndexError::KeyTooLarge {
            index: definition.name.clone(),
            bytes: payload.len(),
            limit: MAX_INCLUDE_BYTES,
        });
    }
    Ok(payload)
}

/// Tree key of an entry: encoded key columns, row id, begin timestamp.
pub fn tree_key(encoded_key: &[u8], row_id: RowId, begin_ts: CommitTs) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded_key.len() + SUFFIX_BYTES);
    out.extend_from_slice(encoded_key);
    out.extend_from_slice(&row_id.0.to_be_bytes());
    out.extend_from_slice(&begin_ts.0.to_be_bytes());
    out
}

/// Tree key prefix of every version of `row_id` under `encoded_key`.
pub fn row_prefix(encoded_key: &[u8], row_id: RowId) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded_key.len() + 16);
    out.extend_from_slice(encoded_key);
    out.extend_from_slice(&row_id.0.to_be_bytes());
    out
}

/// Tree value of an entry: end timestamp and encoded covered values.
pub fn tree_value(end_ts: CommitTs, included: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + included.len());
    out.extend_from_slice(&end_ts.0.to_be_bytes());
    out.extend_from_slice(included);
    out
}

/// The end timestamp stored in a tree value.
pub fn end_of(value: &[u8]) -> Result<CommitTs, IndexError> {
    let bytes: [u8; 8] = value
        .get(..8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| IndexError::Corrupt("index value shorter than its end timestamp".into()))?;
    Ok(CommitTs(u64::from_be_bytes(bytes)))
}

/// The row id and begin timestamp at the end of a tree key.
pub fn suffix_of(key: &[u8]) -> Result<(RowId, CommitTs), IndexError> {
    if key.len() < SUFFIX_BYTES {
        return Err(IndexError::Corrupt(
            "index key shorter than its suffix".into(),
        ));
    }
    let at = key.len() - SUFFIX_BYTES;
    let row: [u8; 16] = key[at..at + 16].try_into().expect("16 bytes");
    let begin: [u8; 8] = key[at + 16..].try_into().expect("8 bytes");
    Ok((
        RowId(u128::from_be_bytes(row)),
        CommitTs(u64::from_be_bytes(begin)),
    ))
}

/// Decodes a stored entry of `definition`.
pub fn decode_entry(
    definition: &IndexDefinition,
    key: &[u8],
    value: &[u8],
) -> Result<IndexEntry, IndexError> {
    let (row_id, begin_ts) = suffix_of(key)?;
    let encoded = &key[..key.len() - SUFFIX_BYTES];
    let (values, used) = decode_key(encoded, &definition.directions())
        .map_err(|error| IndexError::Corrupt(error.to_string()))?;
    if used != encoded.len() {
        return Err(IndexError::Corrupt(
            "trailing bytes after the index key".into(),
        ));
    }
    let end_ts = end_of(value)?;
    let ascending = vec![Direction::Asc; definition.include.len()];
    let (included, used) = decode_key(&value[8..], &ascending)
        .map_err(|error| IndexError::Corrupt(error.to_string()))?;
    if used != value.len() - 8 {
        return Err(IndexError::Corrupt(
            "trailing bytes after the covered values".into(),
        ));
    }
    Ok(IndexEntry {
        key: values,
        row_id,
        begin_ts,
        end_ts,
        included,
    })
}
