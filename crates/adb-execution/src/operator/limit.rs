//! LIMIT: stops pulling once enough rows were produced.
use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError,
};

/// Passes through at most `limit` rows, then stops pulling its input.
pub struct LimitOperator {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Rows still allowed through.
    remaining: usize,
}

impl LimitOperator {
    /// Limits `input` to `limit` rows.
    pub fn new(input: Box<dyn Operator>, limit: usize) -> Self {
        Self {
            input,
            remaining: limit,
        }
    }
}

impl Operator for LimitOperator {
    /// Passes batches through, truncating the last one, then stops pulling.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        if self.remaining == 0 {
            return Ok(None);
        }

        let Some(mut batch) = self.input.next_batch(context)? else {
            return Ok(None);
        };

        if batch.len() > self.remaining {
            batch.truncate(self.remaining);
        }

        self.remaining -= batch.len();
        Ok(Some(batch))
    }

    /// `limit`.
    fn name(&self) -> &'static str {
        "limit"
    }

    /// The input.
    fn children(&self) -> Vec<&dyn Operator> {
        vec![self.input.as_ref()]
    }
}
