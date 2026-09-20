//! Module `filter` for crate `src`.
use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError, Expr,
};

/// Represents `FilterOperator` state used by this subsystem.
pub struct FilterOperator {
    input: Box<dyn Operator>,
    predicate: Expr,
}

/// Implements behavior for `FilterOperator`.
impl FilterOperator {
    /// Implements the `new` operation used by this subsystem.
    pub fn new(input: Box<dyn Operator>, predicate: Expr) -> Self {
        Self { input, predicate }
    }
}

/// Implements behavior for `Operator`.
impl Operator for FilterOperator {
    /// Implements the `next_batch` operation used by this subsystem.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        loop {
            context.check_running()?;
            let Some(batch) = self.input.next_batch(context)? else {
                return Ok(None);
            };

            let mut out = Vec::with_capacity(batch.len());
            for (row_id, row) in batch {
                if self.predicate.evaluate_bool(&row)? {
                    out.push((row_id, row));
                }
            }

            if !out.is_empty() {
                return Ok(Some(out));
            }
        }
    }
}
