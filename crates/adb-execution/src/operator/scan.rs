//! Streaming range scan.
use std::sync::Arc;

use adb_core::{KeyRange, RowId};

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecRow, ExecutionContext, ExecutionError, ScanColumn,
};

/// Emits the rows of a key range one batch at a time; never materializes the whole range.
pub struct ScanOperator {
    /// Where the rows are read from.
    source: Arc<dyn DataSource>,
    /// Key range being scanned.
    range: KeyRange,
    /// Last key returned (keyset cursor).
    after: Option<RowId>,
    /// Whether the source reported the end of the range.
    exhausted: bool,
    /// Stored fields to read and their slots.
    columns: Vec<ScanColumn>,
    /// Slot width of the rows produced.
    width: usize,
    /// Profile name: `scan` (every entity) or `entity_scan`.
    name: &'static str,
    /// Pages requested from the data source.
    pages: u64,
}

impl ScanOperator {
    /// Scans `range` of `source`, writing `columns` into rows of `width` slots; `name` is the
    /// plan operator (`scan` or `entity_scan`).
    pub fn new(
        source: Arc<dyn DataSource>,
        (name, range): (&'static str, KeyRange),
        columns: Vec<ScanColumn>,
        width: usize,
    ) -> Self {
        Self {
            source,
            range,
            after: None,
            exhausted: false,
            columns,
            width,
            name,
            pages: 0,
        }
    }
}

impl Operator for ScanOperator {
    /// Requests the next page after the last returned key; an empty page ends the scan.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        context.check_running()?;
        if self.exhausted {
            return Ok(None);
        }
        self.pages += 1;
        let batch = self.source.scan_page(
            &self.range,
            self.after,
            context.batch_size.max(1),
            context.snapshot_ts,
        )?;
        match batch.last() {
            Some((row_id, _)) => {
                self.after = Some(*row_id);
                Ok(Some(
                    batch
                        .iter()
                        .map(|(row_id, row)| {
                            ExecRow::from_stored(*row_id, row, &self.columns, self.width)
                        })
                        .collect(),
                ))
            }
            None => {
                self.exhausted = true;
                Ok(None)
            }
        }
    }

    /// `scan` or `entity_scan`.
    fn name(&self) -> &'static str {
        self.name
    }

    /// Pages requested from storage.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![("pages", self.pages)]
    }
}
