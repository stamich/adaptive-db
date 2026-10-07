//! Streaming range scan.
use std::sync::Arc;

use adb_core::{KeyRange, RowId};

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecutionContext, ExecutionError,
};

/// Emits the rows of a key range one batch at a time; never materializes the whole range.
pub struct ScanOperator {
    source: Arc<dyn DataSource>,
    range: KeyRange,
    after: Option<RowId>,
    exhausted: bool,
}

impl ScanOperator {
    /// Scans `range` of `source`.
    pub fn new(source: Arc<dyn DataSource>, range: KeyRange) -> Self {
        Self {
            source,
            range,
            after: None,
            exhausted: false,
        }
    }
}

impl Operator for ScanOperator {
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        context.check_running()?;
        if self.exhausted {
            return Ok(None);
        }
        let batch = self.source.scan_page(
            &self.range,
            self.after,
            context.batch_size.max(1),
            context.snapshot_ts,
        )?;
        match batch.last() {
            Some((row_id, _)) => {
                self.after = Some(*row_id);
                Ok(Some(batch))
            }
            None => {
                self.exhausted = true;
                Ok(None)
            }
        }
    }
}
