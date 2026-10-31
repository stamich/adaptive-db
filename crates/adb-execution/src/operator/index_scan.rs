//! Streaming index scan.
use std::sync::Arc;

use adb_core::{FieldId, IndexId};
use adb_index::{IndexBounds, ScanDirection};

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecRow, ExecutionContext, ExecutionError, ScanColumn,
};

/// Emits the entries of an index range one page at a time (keyset paging through the data
/// source), as rows carrying their row id and the requested key and covered columns. Memory
/// is bounded by the page size; the source re-checks on every page that the index still serves
/// the query snapshot.
pub struct IndexScanOperator {
    /// Where the entries are read from.
    source: Arc<dyn DataSource>,
    /// Index scanned.
    index: IndexId,
    /// Equality prefix and range.
    bounds: IndexBounds,
    /// Index order or reversed.
    direction: ScanDirection,
    /// Fields read from the entries and their slots.
    columns: Vec<ScanColumn>,
    /// The fields of `columns`, as passed to the source.
    fields: Vec<FieldId>,
    /// Rows the plan needs at most (sizes pages), if known.
    limit: Option<usize>,
    /// Rows emitted so far.
    emitted: usize,
    /// Position after the last page, or `None` before the first page.
    resume: Option<Vec<u8>>,
    /// Whether the source reported the end of the range.
    exhausted: bool,
    /// Slot width of the rows produced.
    width: usize,
    /// Pages requested from the source.
    pages: u64,
    /// Index entries examined by the source.
    examined: u64,
}

impl IndexScanOperator {
    /// Scans `index` within `bounds` in `direction`, writing `columns` into rows of `width`
    /// slots; `limit` is the most rows the plan will consume, if known.
    pub fn new(
        source: Arc<dyn DataSource>,
        (index, bounds, direction): (IndexId, IndexBounds, ScanDirection),
        columns: Vec<ScanColumn>,
        limit: Option<usize>,
        width: usize,
    ) -> Self {
        let fields = columns.iter().map(|column| column.field_id).collect();
        Self {
            source,
            index,
            bounds,
            direction,
            columns,
            fields,
            limit,
            emitted: 0,
            resume: None,
            exhausted: false,
            width,
            pages: 0,
            examined: 0,
        }
    }
}

impl Operator for IndexScanOperator {
    /// Requests pages until one holds visible entries or the range ends.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        loop {
            context.check_running()?;
            if self.exhausted {
                return Ok(None);
            }
            let wanted = match self.limit {
                Some(limit) => limit
                    .saturating_sub(self.emitted)
                    .clamp(1, context.batch_size.max(1)),
                None => context.batch_size.max(1),
            };
            self.pages += 1;
            let page = self.source.index_scan_page(
                self.index,
                &self.bounds,
                self.direction,
                &self.fields,
                self.resume.as_deref(),
                wanted,
                context.read_view(),
            )?;
            self.examined += page.examined;
            match page.resume {
                Some(resume) => self.resume = Some(resume),
                None => self.exhausted = true,
            }
            if !page.rows.is_empty() {
                self.emitted += page.rows.len();
                return Ok(Some(
                    page.rows
                        .iter()
                        .map(|(row_id, row)| {
                            ExecRow::from_stored(*row_id, row, &self.columns, self.width)
                        })
                        .collect(),
                ));
            }
        }
    }

    /// `index_scan`.
    fn name(&self) -> &'static str {
        "index_scan"
    }

    /// Pages requested and entries examined.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![("pages", self.pages), ("entries_examined", self.examined)]
    }
}
