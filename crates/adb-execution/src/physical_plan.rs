//! Physical plans accepted by the native engine (JSON wire form: docs/plan-wire-format.md).
//!
//! Every node works on [`SlotId`]s. Leaf nodes carry a `columns` list that maps stored
//! `FieldId`s to slots; that mapping is the only place where storage field ids appear.
use adb_core::RowId;
use serde::{Deserialize, Serialize};

use crate::{Expr, ScanColumn, SlotId};

/// One node of a physical plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
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

    /// Equi-join: builds a hash table over `right`, then streams `left` through it.
    ///
    /// Output rows carry the slots of both inputs. With `join_type = left`, a left row without
    /// a match is emitted once with every right slot NULL.
    HashJoin {
        /// Probe side (streamed).
        left: Box<PhysicalPlan>,
        /// Build side (materialized, memory-accounted).
        right: Box<PhysicalPlan>,
        /// INNER or LEFT.
        join_type: JoinType,
        /// Equality conditions `left.slot = right.slot`; NULL keys never match.
        keys: Vec<JoinKey>,
        /// Extra condition evaluated on each key match (part of the join condition, so it
        /// decides LEFT JOIN null-filling, unlike a filter above the join).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        residual: Option<Expr>,
    },

    /// General join: evaluates `predicate` for every pair of rows. Fallback for conditions the
    /// planner cannot express as equality keys; bounded by the comparison limit.
    NestedLoopJoin {
        /// Outer side (streamed).
        left: Box<PhysicalPlan>,
        /// Inner side (materialized, memory-accounted).
        right: Box<PhysicalPlan>,
        /// INNER or LEFT.
        join_type: JoinType,
        /// Join condition; absent means every pair matches (cross join).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        predicate: Option<Expr>,
    },

    /// GROUP BY + aggregate functions (blocking, memory-accounted).
    ///
    /// Output rows carry the `group_by` slots followed by every aggregate's `output` slot.
    /// Without `group_by`, exactly one row is produced (also for empty input).
    Aggregate {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Grouping slots; NULLs form one group.
        group_by: Vec<SlotId>,
        /// Aggregates computed per group.
        aggregates: Vec<AggregateSpec>,
    },

    /// Full sort (blocking, memory-accounted).
    Sort {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Sort keys, most significant first.
        keys: Vec<SortKey>,
    },

    /// The first `limit` rows in `keys` order: `Limit(Sort)` without sorting everything.
    TopK {
        /// Plan producing the rows.
        input: Box<PhysicalPlan>,
        /// Sort keys, most significant first.
        keys: Vec<SortKey>,
        /// Rows kept (at most [`crate::limits::MAX_LIMIT`]).
        limit: usize,
    },
}

/// Aggregate functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFunction {
    /// `COUNT(*)` without input slot, `COUNT(x)` (non-NULL values) with one. Result: INT64.
    Count,
    /// Sum of INT64 (exact, overflow is an error) or FLOAT64 values; NULL for no values.
    Sum,
    /// Smallest value; NULL for no values.
    Min,
    /// Largest value; NULL for no values.
    Max,
    /// Arithmetic mean as FLOAT64; NULL for no values.
    Avg,
}

/// One aggregate of an [`PhysicalPlan::Aggregate`] node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateSpec {
    /// Function to compute.
    pub function: AggregateFunction,
    /// Input slot; only `Count` may omit it (`COUNT(*)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<SlotId>,
    /// Slot the result is written to.
    pub output: SlotId,
}

/// One key of a sort.
///
/// NULLs sort after every value in ascending order and before every value in descending order
/// (PostgreSQL's default `NULLS LAST` / `NULLS FIRST`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SortKey {
    /// Slot compared.
    pub slot: SlotId,
    /// Descending instead of ascending.
    #[serde(default)]
    pub descending: bool,
}

/// Join semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinType {
    /// Only matching pairs.
    Inner,
    /// Matching pairs plus every unmatched left row with NULL right slots.
    Left,
}

/// One equality condition of a hash join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinKey {
    /// Slot produced by the left input.
    pub left: SlotId,
    /// Slot produced by the right input.
    pub right: SlotId,
}

impl PhysicalPlan {
    /// Child plans, left to right.
    pub fn children(&self) -> Vec<&PhysicalPlan> {
        match self {
            Self::PointLookup { .. } | Self::Scan { .. } | Self::EntityScan { .. } => Vec::new(),
            Self::Filter { input, .. }
            | Self::Project { input, .. }
            | Self::Limit { input, .. }
            | Self::Aggregate { input, .. }
            | Self::Sort { input, .. }
            | Self::TopK { input, .. } => {
                vec![input]
            }
            Self::HashJoin { left, right, .. } | Self::NestedLoopJoin { left, right, .. } => {
                vec![left, right]
            }
        }
    }

    /// Slots this node outputs, in output-column order (assumes a validated plan).
    pub fn output_slots(&self) -> Vec<SlotId> {
        match self {
            Self::PointLookup { columns, .. }
            | Self::Scan { columns }
            | Self::EntityScan { columns, .. } => {
                columns.iter().map(|column| column.slot).collect()
            }
            Self::Filter { input, .. }
            | Self::Limit { input, .. }
            | Self::Sort { input, .. }
            | Self::TopK { input, .. } => input.output_slots(),
            Self::Aggregate {
                group_by,
                aggregates,
                ..
            } => group_by
                .iter()
                .copied()
                .chain(aggregates.iter().map(|aggregate| aggregate.output))
                .collect(),
            Self::Project { slots, .. } => slots.clone(),
            Self::HashJoin { left, right, .. } | Self::NestedLoopJoin { left, right, .. } => {
                [left.output_slots(), right.output_slots()].concat()
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
