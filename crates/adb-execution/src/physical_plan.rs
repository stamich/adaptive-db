//! Physical plans accepted by the native engine (JSON wire form: docs/plan-wire-format.md).
//!
//! Every node works on [`SlotId`]s. Leaf nodes carry a `columns` list that maps stored
//! `FieldId`s to slots; that mapping is the only place where storage field ids appear.
use adb_core::RowId;
use serde::{Deserialize, Serialize};

use crate::{Expr, ScanColumn, SlotId};

/// One node of a physical plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PhysicalPlan {
    /// Reads one row by storage key.
    PointLookup {
        /// `(entity << 64) | primary key`; a decimal string on the wire.
        #[serde(with = "row_id_json")]
        row_id: RowId,
        /// Stored fields to read and the slots they are written to.
        columns: Vec<ScanColumn>,
    },

    /// Every row of every entity (diagnostics; prefer `EntityScan`).
    Scan {
        /// Stored fields to read and the slots they are written to.
        columns: Vec<ScanColumn>,
    },

    /// Every row of one entity, read as a key-range scan over `[entity << 64, (entity+1) << 64)`.
    EntityScan {
        /// Entity (table) id.
        entity_id: u64,
        /// Stored fields to read and the slots they are written to.
        columns: Vec<ScanColumn>,
    },

    /// Keeps rows matching a predicate.
    Filter {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Boolean expression; NULL counts as false.
        predicate: Expr,
    },

    /// Selects and orders the output columns.
    Project {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Output slots, in output-column order.
        slots: Vec<SlotId>,
    },

    /// Stops after a number of rows.
    Limit {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Maximum number of rows (at most [`crate::limits::MAX_LIMIT`]).
        limit: usize,
    },
}

impl PhysicalPlan {
    /// Child plans, left to right.
    pub fn children(&self) -> Vec<&PhysicalPlan> {
        match self {
            Self::PointLookup { .. } | Self::Scan { .. } | Self::EntityScan { .. } => Vec::new(),
            Self::Filter { input, .. }
            | Self::Project { input, .. }
            | Self::Limit { input, .. } => {
                vec![input]
            }
        }
    }
}

/// JSON adapter for 128-bit row identifiers.
///
/// JSON numbers are not a portable representation for the complete `u128` domain used by
/// composed `(entity_id << 64) | primary_key` identifiers. The canonical wire form is a
/// decimal string; small legacy numeric values are still accepted on input.
mod row_id_json {
    use adb_core::RowId;
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    /// Serializes a row id as a decimal JSON string.
    pub fn serialize<S>(row_id: &RowId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&row_id.0.to_string())
    }

    /// Deserializes the canonical string form while retaining compatibility with small JSON numbers.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<RowId, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Accepts the canonical decimal-string wire form and the legacy small numeric form.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireRowId {
            /// Canonical form: the full `u128` as a decimal string.
            Decimal(String),
            /// Legacy form: a JSON number that fits in `u64`.
            LegacyNumber(u64),
        }

        match WireRowId::deserialize(deserializer)? {
            WireRowId::Decimal(value) => value.parse::<u128>().map(RowId).map_err(D::Error::custom),
            WireRowId::LegacyNumber(value) => Ok(RowId(u128::from(value))),
        }
    }
}
