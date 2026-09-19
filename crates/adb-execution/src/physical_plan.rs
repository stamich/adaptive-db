//! Module `physical_plan` for crate `adb-execution`.
use adb_core::{FieldId, RowId};
use serde::{Deserialize, Serialize};

use crate::Expr;

/// Enumerates `PhysicalPlan` alternatives used by this subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PhysicalPlan {
    PointLookup {
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
