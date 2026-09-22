//! Module `physical_plan` for crate `adb-execution`.
use adb_core::{FieldId, RowId};
use serde::{Deserialize, Serialize};

use crate::Expr;

/// Enumerates `PhysicalPlan` alternatives used by this subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PhysicalPlan {
    PointLookup {
        #[serde(with = "row_id_json")]
        row_id: RowId,
    },

    Scan,

    Filter {
        input: Box<PhysicalPlan>,
        predicate: Expr,
    },

    Project {
        input: Box<PhysicalPlan>,
        fields: Vec<FieldId>,
    },

    Limit {
        input: Box<PhysicalPlan>,
        limit: usize,
    },
}

/// Implements behavior for `PhysicalPlan`.
impl PhysicalPlan {
    /// Implements the `validate` operation used by this subsystem.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_depth(0)
    }

    /// Implements the `validate_depth` operation used by this subsystem.
    fn validate_depth(&self, depth: usize) -> Result<(), String> {
        /// Defines the `MAX_PLAN_DEPTH` constant used by this subsystem.
        const MAX_PLAN_DEPTH: usize = 128;
        if depth > MAX_PLAN_DEPTH {
            return Err(format!("physical plan depth exceeds {MAX_PLAN_DEPTH}"));
        }

        match self {
            Self::PointLookup { .. } | Self::Scan => Ok(()),
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
