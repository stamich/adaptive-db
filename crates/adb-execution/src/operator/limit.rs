//! LIMIT: stops pulling once enough rows were produced.
use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError,
};

/// Passes through at most `limit` rows, then stops pulling its input.
pub struct LimitOperator {
    input: Box<dyn Operator>,
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
}
