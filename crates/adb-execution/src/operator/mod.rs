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
pub mod project;
pub mod scan;
pub mod sort;
pub mod top_k;

pub use crate::RowBatch;

use crate::{ExecutionContext, ExecutionError};

/// A pull-based operator.
pub trait Operator: Send {
    /// Next batch, or `None` when the operator is exhausted. Never returns an empty batch.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError>;
}
