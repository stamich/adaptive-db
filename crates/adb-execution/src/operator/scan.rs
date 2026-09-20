//! Module `scan` for crate `src`.
use std::sync::Arc;

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecutionContext, ExecutionError,
};

/// Represents `ScanOperator` state used by this subsystem.
pub struct ScanOperator {
    source: Arc<dyn DataSource>,
    rows: Option<Vec<(adb_core::RowId, adb_core::Row)>>,
    offset: usize,
}

/// Implements behavior for `ScanOperator`.
impl ScanOperator {
    /// Implements the `new` operation used by this subsystem.
    pub fn new(source: Arc<dyn DataSource>) -> Self {
        Self {
            source,
            rows: None,
            offset: 0,
        }
    }
}

/// Implements behavior for `Operator`.
impl Operator for ScanOperator {
    /// Implements the `next_batch` operation used by this subsystem.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        context.check_running()?;

        if self.rows.is_none() {
            self.rows = Some(self.source.scan_rows(context.snapshot_ts)?);
        }

        let Some(rows) = self.rows.as_ref() else {
            return Err(ExecutionError::InvalidPlan(
                "scan source did not initialize".into(),
            ));
        };
        if self.offset >= rows.len() {
            return Ok(None);
        }

        let end = self
            .offset
            .saturating_add(context.batch_size.max(1))
            .min(rows.len());

        let batch = rows[self.offset..end].to_vec();
        self.offset = end;
        Ok(Some(batch))
    }
}
