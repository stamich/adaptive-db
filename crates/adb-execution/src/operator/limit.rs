//! Module `limit` for crate `src`.
use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError,
};

/// Represents `LimitOperator` state used by this subsystem.
pub struct LimitOperator {
    input: Box<dyn Operator>,
    remaining: usize,
}

/// Implements behavior for `LimitOperator`.
impl LimitOperator {
    /// Implements the `new` operation used by this subsystem.
    pub fn new(input: Box<dyn Operator>, limit: usize) -> Self {
        Self {
            input,
            remaining: limit,
        }
    }
}

/// Implements behavior for `Operator`.
impl Operator for LimitOperator {
    /// Implements the `next_batch` operation used by this subsystem.
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
