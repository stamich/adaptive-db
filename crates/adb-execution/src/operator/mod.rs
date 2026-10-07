//! Physical operators. Each pulls batches from its input on demand.
pub mod filter;
pub mod limit;
pub mod point_lookup;
pub mod project;
pub mod scan;

use adb_core::{Row, RowId};

use crate::{ExecutionContext, ExecutionError};

/// Row-oriented batch exchanged between operators.
pub type RowBatch = Vec<(RowId, Row)>;

/// A pull-based operator.
pub trait Operator: Send {
    /// Next batch, or `None` when the operator is exhausted.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError>;
}
