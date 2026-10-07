//! Physical plans accepted by the native engine (JSON wire form, see docs/plan-wire-format.md).
use adb_core::{FieldId, RowId};
use serde::{Deserialize, Serialize};

use crate::Expr;

/// One node of a physical plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PhysicalPlan {
    /// Reads one row by storage key.
    PointLookup {
        /// `(entity << 64) | primary key`; a decimal string on the wire.
        #[serde(with = "row_id_json")]
        row_id: RowId,
    },

    /// Every row of every entity (diagnostics; prefer `EntityScan`).
    Scan,

    /// Every row of one entity, read as a key-range scan over `(entity << 64, (entity+1) << 64)`.
    EntityScan {
        /// Entity (table) id.
        entity_id: u64,
    },

    /// Keeps rows matching a predicate.
    Filter {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Boolean expression; NULL counts as false.
        predicate: Expr,
    },

    /// Keeps only some fields.
    Project {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Fields to keep.
        fields: Vec<FieldId>,
    },

    /// Stops after a number of rows.
    Limit {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Maximum number of rows.
        limit: usize,
    },
}

impl PhysicalPlan {
    /// Rejects plans and expressions beyond the hardened depth/size limits.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_depth(0)
    }

    /// Depth-bounded validation helper.
    fn validate_depth(&self, depth: usize) -> Result<(), String> {
        /// Deepest plan accepted.
        const MAX_PLAN_DEPTH: usize = 128;
        if depth > MAX_PLAN_DEPTH {
            return Err(format!("physical plan depth exceeds {MAX_PLAN_DEPTH}"));
        }

        match self {
            Self::PointLookup { .. } | Self::Scan | Self::EntityScan { .. } => Ok(()),
            Self::Filter { input, predicate } => {
                predicate.validate()?;
                input.validate_depth(depth + 1)
            }
            Self::Project { input, fields } => {
                if fields.len() > 4096 {
                    return Err("too many projected fields".to_string());
                }
                input.validate_depth(depth + 1)
            }
            Self::Limit { input, .. } => input.validate_depth(depth + 1),
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
            Decimal(String),
            LegacyNumber(u64),
        }

        match WireRowId::deserialize(deserializer)? {
            WireRowId::Decimal(value) => value.parse::<u128>().map(RowId).map_err(D::Error::custom),
            WireRowId::LegacyNumber(value) => Ok(RowId(u128::from(value))),
        }
    }
}
