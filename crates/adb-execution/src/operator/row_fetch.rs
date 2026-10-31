//! Batched row fetch below an index scan.
use std::sync::Arc;

use adb_core::RowId;

use crate::{
    operator::{Operator, RowBatch},
    DataSource, ExecutionContext, ExecutionError, ScanColumn,
};

/// Reads the stored rows of its input's row ids and adds `columns` to each input row.
///
/// The ids of one input batch are sorted and fetched in one call, so the data source visits
/// each storage page once per batch; the rows are emitted in input (index) order.
pub struct RowFetchOperator {
    /// Rows with row ids.
    input: Box<dyn Operator>,
    /// Where the rows are read from.
    source: Arc<dyn DataSource>,
    /// Stored fields to read and their slots.
    columns: Vec<ScanColumn>,
    /// Batches fetched.
    batches: u64,
    /// Rows fetched.
    fetched: u64,
}

impl RowFetchOperator {
    /// Fetches `columns` for the rows of `input` from `source`.
    pub fn new(
        input: Box<dyn Operator>,
        source: Arc<dyn DataSource>,
        columns: Vec<ScanColumn>,
    ) -> Self {
        Self {
            input,
            source,
            columns,
            batches: 0,
            fetched: 0,
        }
    }
}

impl Operator for RowFetchOperator {
    /// Fetches the rows of the next input batch.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        let Some(mut rows) = self.input.next_batch(context)? else {
            return Ok(None);
        };
        let mut ids: Vec<RowId> = rows
            .iter()
            .map(|row| {
                row.row_id.ok_or_else(|| {
                    ExecutionError::InvalidPlan("row fetch input rows need row ids".into())
                })
            })
            .collect::<Result<_, _>>()?;
        ids.sort_unstable();
        ids.dedup();
        let stored = self.source.fetch_rows(&ids, context.read_view())?;
        self.batches += 1;
        self.fetched += ids.len() as u64;
        for row in &mut rows {
            let row_id = row.row_id.expect("checked above");
            let at = ids.binary_search(&row_id).expect("every id was fetched");
            let Some(found) = &stored[at] else {
                return Err(ExecutionError::DataSource(format!(
                    "index entry points at {row_id:?}, which is not visible at the snapshot; \
                     the index may be damaged (rebuild the projections)"
                )));
            };
            for column in &self.columns {
                if let Some(value) = found.get(column.field_id) {
                    row.set(column.slot, value.clone());
                }
            }
        }
        Ok(Some(rows))
    }

    /// `row_fetch`.
    fn name(&self) -> &'static str {
        "row_fetch"
    }

    /// The input operator.
    fn children(&self) -> Vec<&dyn Operator> {
        vec![self.input.as_ref()]
    }

    /// Fetch batches and rows fetched.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("fetch_batches", self.batches),
            ("rows_fetched", self.fetched),
        ]
    }
}
