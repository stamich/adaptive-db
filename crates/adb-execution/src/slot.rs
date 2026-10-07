//! Execution slots: the column identity used by every operator of a query.
//!
//! Storage addresses a value by `(entity, FieldId)`. A query may read the same entity twice
//! (self-join, aliases), so field ids are not unique inside one operator row. The JVM binder
//! therefore assigns every column of every relation *instance* its own [`SlotId`]; scans map
//! `FieldId -> SlotId` once and everything above them works on slots only.
use std::fmt;

use serde::{Deserialize, Serialize};

/// Dense, query-scoped column identifier (`0 ..` [`crate::limits::MAX_SLOTS`]).
///
/// A slot is also the index of the value inside an [`crate::ExecRow`], and the column id
/// written to the batch wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlotId(pub u32);

impl SlotId {
    /// Position of the slot inside an [`crate::ExecRow`].
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for SlotId {
    /// Renders as `#<n>`, the notation used by EXPLAIN and error messages.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Binding of one stored field to the slot a scan writes it into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanColumn {
    /// Field read from the stored row.
    pub field_id: adb_core::FieldId,
    /// Slot the value is written to.
    pub slot: SlotId,
}
