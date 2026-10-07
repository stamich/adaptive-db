//! Batched emission of rows a blocking operator has finished computing.
use crate::{ExecRow, RowBatch};

/// Rows produced by a blocking operator, handed out `batch_size` at a time.
pub struct OutputBuffer {
    /// Remaining rows in output order.
    rows: std::vec::IntoIter<ExecRow>,
}

impl OutputBuffer {
    /// Emits `rows` in order.
    pub fn new(rows: Vec<ExecRow>) -> Self {
        Self {
            rows: rows.into_iter(),
        }
    }

    /// Next batch of at most `batch_size` rows, or `None` when everything was emitted.
    pub fn next_batch(&mut self, batch_size: usize) -> Option<RowBatch> {
        let batch: RowBatch = self.rows.by_ref().take(batch_size.max(1)).collect();
        (!batch.is_empty()).then_some(batch)
    }
}
