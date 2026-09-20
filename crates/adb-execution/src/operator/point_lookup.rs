//! Module `point_lookup` for crate `src`.
use std::sync::Arc;

use adb_core::RowId;

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecutionContext, ExecutionError,
};

/// Represents `PointLookupOperator` state used by this subsystem.
pub struct PointLookupOperator {
    source: Arc<dyn DataSource>,
    row_id: RowId,
    done: bool,
}

/// Implements behavior for `PointLookupOperator`.
impl PointLookupOperator {
    /// Implements the `new` operation used by this subsystem.
    pub fn new(source: Arc<dyn DataSource>, row_id: RowId) -> Self {
        Self {
            source,
            row_id,
            done: false,
        }
    }
}

/// Implements behavior for `Operator`.
impl Operator for PointLookupOperator {
    /// Implements the `next_batch` operation used by this subsystem.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        context.check_running()?;
        if self.done {
            return Ok(None);
        }
        self.done = true;

        Ok(self
            .source
            .point_lookup(self.row_id, context.snapshot_ts)?
            .map(|row| vec![(self.row_id, row)]))
    }
}
