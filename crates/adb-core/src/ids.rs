//! Strongly typed identifiers shared by every engine layer.
use serde::{Deserialize, Serialize};

/// Physical storage key of one logical row.
///
/// Since Milestone 2 the key is composed as `(entity_id << 64) | primary_key`, so all rows of one
/// entity occupy one contiguous key range. Range scans over an entity (see [`crate::KeyRange`])
/// and change-data-capture both rely on this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RowId(pub u128);

impl RowId {
    /// Composes the storage key of `primary_key` within `entity_id`.
    pub const fn compose(entity_id: u64, primary_key: u64) -> Self {
        Self(((entity_id as u128) << 64) | primary_key as u128)
    }

    /// Returns the entity (table) component of the key.
    pub const fn entity_id(self) -> u64 {
        (self.0 >> 64) as u64
    }

    /// Returns the primary-key component of the key.
    pub const fn primary_key(self) -> u64 {
        self.0 as u64
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

/// Index of a fixed-size page inside one page file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PageId(pub u64);

/// Index of a tuple slot inside one slotted heap page.
pub type SlotId = u16;
/// Column identifier inside a row.
pub type FieldId = u32;

#[cfg(test)]
mod tests {
    use super::RowId;

    #[test]
    fn row_id_round_trips_its_components() {
        let id = RowId::compose(7, u64::MAX);
        assert_eq!(id.entity_id(), 7);
        assert_eq!(id.primary_key(), u64::MAX);
        assert!(RowId::compose(7, u64::MAX) < RowId::compose(8, 0));
    }
}
