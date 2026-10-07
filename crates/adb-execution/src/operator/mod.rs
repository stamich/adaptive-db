//! Physical operators. Each pulls batches from its input on demand.
pub mod aggregate;
pub mod collect;
pub mod filter;
pub mod hash_join;
pub mod join;
pub mod limit;
pub mod nested_loop_join;
pub mod ordering;
pub mod output_buffer;
pub mod point_lookup;
pub mod profiled;
pub mod project;
pub mod scan;
pub mod sort;
pub mod top_k;

pub use crate::RowBatch;

use crate::{profile::OperatorProfile, ExecutionContext, ExecutionError};

/// A pull-based operator.
pub trait Operator: Send {
    /// Next batch, or `None` when the operator is exhausted. Never returns an empty batch.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError>;

    /// Operator name used in profiles (the plan wire `op` tag).
    fn name(&self) -> &'static str;

    /// Input operators, left to right.
    fn children(&self) -> Vec<&dyn Operator> {
        Vec::new()
    }

    /// Operator-specific counters reported in the profile.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        Vec::new()
    }

    /// Profile of this operator and its inputs (rows and time are filled in by
    /// [`profiled::Profiled`]).
    fn profile(&self) -> OperatorProfile {
        OperatorProfile::new(
            self.name(),
            self.counters(),
            self.children()
                .into_iter()
                .map(|child| child.profile())
                .collect(),
        )
    }
}
