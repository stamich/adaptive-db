//! Physical operators. Each pulls batches from its input on demand.
pub mod filter;
pub mod limit;
pub mod point_lookup;
pub mod project;
pub mod scan;

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
