//! Single-row lookup by storage key.
use std::sync::Arc;

use adb_core::RowId;

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecutionContext, ExecutionError,
};

/// Emits at most one row: `row_id` as of the query snapshot.
pub struct PointLookupOperator {
    /// Where the row is read from.
    source: Arc<dyn DataSource>,
    /// Key of the row.
    row_id: RowId,
    /// Whether the lookup already ran.
    done: bool,
}

impl PointLookupOperator {
    /// Looks up `row_id` in `source`.
    pub fn new(source: Arc<dyn DataSource>, row_id: RowId) -> Self {
        Self {
            source,
            row_id,
            done: false,
        }
    }
}

impl Operator for PointLookupOperator {
    /// Returns the row once (if visible), then `None`.
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
