//! Runtime profile of a query: what every operator actually did.
//!
//! This is the observation half of Adaptive DB's adaptive loop. The planner decides (and
//! explains why) before execution; the profile reports afterwards how much each operator
//! read, produced, held in memory and compared. EXPLAIN ANALYZE shows it today; the
//! statistics and advisor milestones (2.2, 5.x) are meant to consume the same structure.
use std::{collections::BTreeMap, time::Duration};

use serde::Serialize;

/// Measurements of one operator and, recursively, its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperatorProfile {
    /// Operator name (`hash_join`, `entity_scan`, ...), matching the plan wire `op` tags.
    pub operator: &'static str,
    /// Rows the operator returned.
    pub rows_out: u64,
    /// Batches the operator returned.
    pub batches_out: u64,
    /// Wall-clock time spent in the operator *including* its inputs, in microseconds.
    pub elapsed_us: u64,
    /// Operator-specific counters (e.g. `build_rows`, `comparisons`, `peak_memory_bytes`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub counters: BTreeMap<&'static str, u64>,
    /// Profiles of the inputs, left to right.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OperatorProfile>,
}

impl OperatorProfile {
    /// A profile with no measurements yet.
    pub fn new(
        operator: &'static str,
        counters: Vec<(&'static str, u64)>,
        children: Vec<OperatorProfile>,
    ) -> Self {
        Self {
            operator,
            rows_out: 0,
            batches_out: 0,
            elapsed_us: 0,
            counters: counters.into_iter().collect(),
            children,
        }
    }

    /// Rows produced by the leaves (rows read from storage).
    pub fn leaf_rows(&self) -> u64 {
        if self.children.is_empty() {
            self.rows_out
        } else {
            self.children.iter().map(OperatorProfile::leaf_rows).sum()
        }
    }
}

/// Profile of a whole query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryProfile {
    /// Highest number of bytes the query's blocking operators held at once.
    pub peak_memory_bytes: u64,
    /// The query's memory budget.
    pub memory_limit_bytes: u64,
    /// The operator tree.
    pub root: OperatorProfile,
}

/// Saturating conversion of a duration to whole microseconds.
pub(crate) fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
