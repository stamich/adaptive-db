//! Single-row lookup by storage key.
use std::sync::Arc;

use adb_core::RowId;

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecRow, ExecutionContext, ExecutionError, ScanColumn,
};

/// Emits at most one row: `row_id` as of the query snapshot.
pub struct PointLookupOperator {
    /// Where the row is read from.
    source: Arc<dyn DataSource>,
    /// Key of the row.
    row_id: RowId,
    /// Whether the lookup already ran.
    done: bool,
    /// Stored fields to read and their slots.
    columns: Vec<ScanColumn>,
    /// Slot width of the row produced.
    width: usize,
}

impl PointLookupOperator {
    /// Looks up `row_id` in `source`, writing `columns` into a row of `width` slots.
    pub fn new(
        source: Arc<dyn DataSource>,
        row_id: RowId,
        columns: Vec<ScanColumn>,
        width: usize,
    ) -> Self {
        Self {
            source,
            row_id,
            done: false,
            columns,
            width,
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
            .map(|row| {
                vec![ExecRow::from_stored(
                    self.row_id,
                    &row,
                    &self.columns,
                    self.width,
                )]
            }))
    }
}
