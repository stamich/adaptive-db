//! Row filter.
use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError, Expr,
};

/// Keeps rows for which the predicate is true; never returns an empty batch.
pub struct FilterOperator {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Rows for which this is true are kept.
    predicate: Expr,
}

impl FilterOperator {
    /// Filters `input` with `predicate`.
    pub fn new(input: Box<dyn Operator>, predicate: Expr) -> Self {
        Self { input, predicate }
    }
}

impl Operator for FilterOperator {
    /// Pulls input batches until one has matching rows or the input ends.
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
