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
    /// Rows evaluated (the profile's selectivity is `rows_out / rows_in`).
    rows_in: u64,
}

impl FilterOperator {
    /// Filters `input` with `predicate`.
    pub fn new(input: Box<dyn Operator>, predicate: Expr) -> Self {
        Self {
            input,
            predicate,
            rows_in: 0,
        }
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

            self.rows_in += batch.len() as u64;
            let mut out = Vec::with_capacity(batch.len());
            for row in batch {
                if self.predicate.evaluate_bool(&row)? {
                    out.push(row);
                }
            }

            if !out.is_empty() {
                return Ok(Some(out));
            }
        }
    }

    /// `filter`.
    fn name(&self) -> &'static str {
        "filter"
    }

    /// The input.
    fn children(&self) -> Vec<&dyn Operator> {
        vec![self.input.as_ref()]
    }

    /// Rows evaluated.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![("rows_in", self.rows_in)]
    }
}
