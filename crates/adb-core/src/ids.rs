//! Strongly typed identifiers shared by every engine layer.
use serde::{Deserialize, Serialize};

/// Sign bit of a 64-bit primary key.
const SIGN: u64 = 1 << 63;

/// Physical storage key of one logical row.
///
/// Since Milestone 2 the key is composed as `(entity_id << 64) | key_bits`, so all rows of one
/// entity occupy one contiguous key range. Range scans over an entity (see [`crate::KeyRange`])
/// and change-data-capture both rely on this layout.
///
/// Since data format 2 (Milestone 2.3.0) the `key_bits` of a SQL `BIGINT` primary key are
/// the key with its sign bit flipped ([`RowId::from_primary_key`]), so key order is value order:
/// `-1` sorts before `0` and `i64::MIN` first. Data format 1 stored the raw two's-complement bits,
/// which sorted negative keys after every positive one; `adb-migrate` rewrites such databases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RowId(pub u128);

impl RowId {
    /// Composes a storage key from raw key bits (no sign handling; see
    /// [`RowId::from_primary_key`] for SQL primary keys).
    pub const fn compose(entity_id: u64, key_bits: u64) -> Self {
        Self(((entity_id as u128) << 64) | key_bits as u128)
    }

    /// The order-preserving storage key of the SQL primary key `primary_key` within `entity_id`.
    pub const fn from_primary_key(entity_id: u64, primary_key: i64) -> Self {
        Self::compose(entity_id, (primary_key as u64) ^ SIGN)
    }

    /// Returns the entity (table) component of the key.
    pub const fn entity_id(self) -> u64 {
        (self.0 >> 64) as u64
    }

    /// Returns the raw key bits (the low 64 bits).
    pub const fn primary_key(self) -> u64 {
        self.0 as u64
    }

    /// Returns the SQL primary key of a key built by [`RowId::from_primary_key`].
    pub const fn signed_primary_key(self) -> i64 {
        ((self.0 as u64) ^ SIGN) as i64
    }
}

/// Identifier of one transaction attempt; unique for the lifetime of a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TxId(pub u64);

/// Logical MVCC commit timestamp; snapshots are expressed in the same unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CommitTs(pub u64);

/// Position in the canonical log (`segment << 32 | offset`); totally ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lsn(pub u64);

/// Identifier of a secondary index; unique for the lifetime of a database (never reused).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IndexId(pub u64);

/// Index of a fixed-size page inside one page file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PageId(pub u64);

/// Index of a tuple slot inside one slotted heap page (not to be confused with the execution
/// `adb_execution::SlotId`, which names a column of an operator row).
pub type TupleSlot = u16;
/// Column identifier inside a row.
pub type FieldId = u32;

/// Unit tests of the identifier helpers.
#[cfg(test)]
mod tests {
    use super::RowId;

    /// Composing and splitting a row id is lossless and preserves entity order.
    #[test]
    fn row_id_round_trips_its_components() {
        let id = RowId::compose(7, u64::MAX);
        assert_eq!(id.entity_id(), 7);
        assert_eq!(id.primary_key(), u64::MAX);
        assert!(RowId::compose(7, u64::MAX) < RowId::compose(8, 0));
    }

    /// Primary keys round-trip and their storage keys sort in value order.
    #[test]
    fn primary_keys_sort_in_value_order() {
        let keys = [i64::MIN, -5, -1, 0, 1, 42, i64::MAX];
        let ids: Vec<RowId> = keys
            .iter()
            .map(|k| RowId::from_primary_key(3, *k))
            .collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        for (key, id) in keys.iter().zip(&ids) {
            assert_eq!(id.signed_primary_key(), *key);
            assert_eq!(id.entity_id(), 3);
        }
        assert!(RowId::from_primary_key(3, i64::MAX) < RowId::from_primary_key(4, i64::MIN));
    }
}
