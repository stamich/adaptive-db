//! Value-level scan bounds and their translation into byte ranges of the index tree.
//!
//! The planner describes an index scan in SQL terms: equality values for a prefix of the key
//! columns, then optionally a range on the next column. [`IndexBounds::byte_range`] turns that
//! into one range of tree keys, which is exact: the scan returns precisely the entries whose key
//! matches (no residual check is needed for these columns).
//!
//! Rules (see `adb_core::key_encoding` for the encoding):
//! * the entries whose key starts with the encoding `e` of some leading values form the range
//!   `[e, prefix_successor(e))`, whatever follows (more columns, row id, timestamp);
//! * a comparison never matches NULL, so a range on a column is clamped to its non-NULL keys:
//!   ascending NULL encodes as the tag `0xF0` after every value, descending as `0x0F` before
//!   every value;
//! * on a descending column, value order is the reverse of byte order, so a lower value bound
//!   becomes an upper byte bound and vice versa.
use std::ops::Bound;

use adb_core::{
    encode_key, encode_value, key_encoding::TAG_NULL, prefix_successor, Direction, Value,
};
use serde::{Deserialize, Serialize};

use crate::{IndexDefinition, IndexError};

/// One end of a range on a key column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueBound {
    /// The bounding value (never NULL).
    pub value: Value,
    /// Whether the value itself is included.
    pub inclusive: bool,
}

/// Which entries an index scan reads, in terms of key column values.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexBounds {
    /// Values the leading key columns must equal (`col1 = v1 AND col2 = v2 ...`).
    #[serde(default)]
    pub prefix: Vec<Value>,
    /// Lower bound on the key column after the prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower: Option<ValueBound>,
    /// Upper bound on the key column after the prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper: Option<ValueBound>,
}

/// A range of tree keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteRange {
    /// Lower end.
    pub lower: Bound<Vec<u8>>,
    /// Upper end.
    pub upper: Bound<Vec<u8>>,
}

impl ByteRange {
    /// The whole tree.
    pub fn all() -> Self {
        Self {
            lower: Bound::Unbounded,
            upper: Bound::Unbounded,
        }
    }

    /// Whether `key` lies in the range.
    pub fn contains(&self, key: &[u8]) -> bool {
        let above = match &self.lower {
            Bound::Included(bound) => key >= bound.as_slice(),
            Bound::Excluded(bound) => key > bound.as_slice(),
            Bound::Unbounded => true,
        };
        let below = match &self.upper {
            Bound::Included(bound) => key <= bound.as_slice(),
            Bound::Excluded(bound) => key < bound.as_slice(),
            Bound::Unbounded => true,
        };
        above && below
    }
}

impl IndexBounds {
    /// Bounds that read the whole index.
    pub fn all() -> Self {
        Self::default()
    }

    /// Checks the bounds against `definition`: no more prefix values than key columns, a range
    /// only on a column that exists, no NULL anywhere.
    pub fn validate(&self, definition: &IndexDefinition) -> Result<(), IndexError> {
        let fail = |message: String| Err(IndexError::InvalidDefinition(message));
        let ranged = self.lower.is_some() || self.upper.is_some();
        let needed = self.prefix.len() + usize::from(ranged);
        if needed > definition.columns.len() {
            return fail(format!(
                "index {} has {} key columns; the bounds address {needed}",
                definition.name,
                definition.columns.len()
            ));
        }
        let values = self
            .prefix
            .iter()
            .chain(self.lower.iter().map(|bound| &bound.value))
            .chain(self.upper.iter().map(|bound| &bound.value));
        if values.into_iter().any(|value| matches!(value, Value::Null)) {
            return fail("index bounds never compare with NULL".into());
        }
        Ok(())
    }

    /// The range of tree keys holding exactly the matching entries of `definition`.
    pub fn byte_range(&self, definition: &IndexDefinition) -> Result<ByteRange, IndexError> {
        self.validate(definition)?;
        let prefix = encode_key(
            self.prefix
                .iter()
                .zip(definition.columns.iter().map(|c| c.direction)),
        );
        if self.lower.is_none() && self.upper.is_none() {
            return Ok(if prefix.is_empty() {
                ByteRange::all()
            } else {
                ByteRange {
                    upper: upper_after(&prefix),
                    lower: Bound::Included(prefix),
                }
            });
        }
        let direction = definition.columns[self.prefix.len()].direction;
        // The non-NULL keys of the ranged column under the prefix.
        let (null_free_lower, null_free_upper) = match direction {
            Direction::Asc => (
                Bound::Included(prefix.clone()),
                Bound::Excluded(with(&prefix, TAG_NULL)),
            ),
            // Descending NULL keys start with `prefix | 0x0F`; the first non-NULL key is above
            // every one of them.
            Direction::Desc => (
                Bound::Included(with(&prefix, !TAG_NULL + 1)),
                upper_after(&prefix),
            ),
        };
        let encoded = |bound: &ValueBound| {
            let mut out = prefix.clone();
            encode_value(&bound.value, direction, &mut out);
            out
        };
        // Byte bound that starts at the keys of `e` (inclusive) or after them (exclusive).
        let from = |e: Vec<u8>, inclusive: bool| -> Bound<Vec<u8>> {
            if inclusive {
                Bound::Included(e)
            } else {
                match prefix_successor(&e) {
                    Some(next) => Bound::Included(next),
                    None => Bound::Excluded(e),
                }
            }
        };
        // Byte bound that ends after the keys of `e` (inclusive) or before them (exclusive).
        let to = |e: Vec<u8>, inclusive: bool| -> Bound<Vec<u8>> {
            if inclusive {
                upper_after(&e)
            } else {
                Bound::Excluded(e)
            }
        };
        let (lower, upper) = match direction {
            Direction::Asc => (
                self.lower.as_ref().map(|b| from(encoded(b), b.inclusive)),
                self.upper.as_ref().map(|b| to(encoded(b), b.inclusive)),
            ),
            Direction::Desc => (
                self.upper.as_ref().map(|b| from(encoded(b), b.inclusive)),
                self.lower.as_ref().map(|b| to(encoded(b), b.inclusive)),
            ),
        };
        Ok(ByteRange {
            lower: lower.unwrap_or(null_free_lower),
            upper: upper.unwrap_or(null_free_upper),
        })
    }
}

/// Exclusive upper bound after every key starting with `prefix` (unbounded if none exists).
fn upper_after(prefix: &[u8]) -> Bound<Vec<u8>> {
    prefix_successor(prefix).map_or(Bound::Unbounded, Bound::Excluded)
}

/// `prefix` followed by one byte.
fn with(prefix: &[u8], byte: u8) -> Vec<u8> {
    let mut out = prefix.to_vec();
    out.push(byte);
    out
}
