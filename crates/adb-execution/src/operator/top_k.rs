//! TopK: the first `k` rows of a sort without sorting the whole input.
use crate::{
    memory::MemoryReservation,
    operator::{ordering::sort_rows, output_buffer::OutputBuffer, Operator, RowBatch},
    ExecRow, ExecutionContext, ExecutionError, SortKey,
};

/// Streams its input into a buffer of at most `2k` rows (and at most the materialized-row cap);
/// whenever the buffer is full it is sorted and cut back to `k`. Memory is O(k) instead of O(input) and the work is
/// O(n log k) amortized. Ties keep input order, exactly like `Limit(Sort)`.
pub struct TopKOperator {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Sort keys, most significant first.
    keys: Vec<SortKey>,
    /// Rows kept.
    k: usize,
    /// Final rows, once computed.
    output: Option<OutputBuffer>,
    /// Memory of the buffered rows (held until the operator is dropped).
    reservation: Option<MemoryReservation>,
    /// Rows read from the input.
    rows_in: u64,
    /// Sort-and-truncate passes performed.
    compactions: u64,
}

impl TopKOperator {
    /// Keeps the first `k` rows of `input` in `keys` order.
    pub fn new(input: Box<dyn Operator>, keys: Vec<SortKey>, k: usize) -> Self {
        Self {
            input,
            keys,
            k,
            output: None,
            reservation: None,
            rows_in: 0,
            compactions: 0,
        }
    }

    /// Sorts `buffer` and truncates it to `k` rows, returning the freed memory.
    fn compact(
        &mut self,
        buffer: &mut Vec<ExecRow>,
        reservation: &mut MemoryReservation,
    ) -> Result<(), ExecutionError> {
        self.compactions += 1;
        sort_rows(buffer, &self.keys)?;
        let dropped: usize = buffer[self.k.min(buffer.len())..]
            .iter()
            .map(ExecRow::estimated_bytes)
            .sum();
        buffer.truncate(self.k);
        reservation.shrink(dropped);
        Ok(())
    }

    /// Consumes the input and returns the final `k` rows.
    fn compute(&mut self, context: &ExecutionContext) -> Result<Vec<ExecRow>, ExecutionError> {
        let mut reservation = context.memory.reservation("top-k");
        let mut buffer: Vec<ExecRow> = Vec::new();
        let max_rows = context.limits.max_materialized_rows;
        if self.k > max_rows {
            return Err(ExecutionError::ResourceLimit(format!(
                "top-k would hold {} rows, more than {max_rows}",
                self.k
            )));
        }
        if self.k > 0 {
            // The buffer never exceeds the materialized-row cap (k + 1 when k equals the cap).
            let capacity = self.k.saturating_mul(2).min(max_rows).max(self.k + 1);
            while let Some(batch) = self.input.next_batch(context)? {
                context.check_running()?;
                self.rows_in += batch.len() as u64;
                for row in batch {
                    reservation.grow(row.estimated_bytes())?;
                    buffer.push(row);
                    if buffer.len() >= capacity {
                        self.compact(&mut buffer, &mut reservation)?;
                    }
                }
            }
            self.compact(&mut buffer, &mut reservation)?;
        }
        self.reservation = Some(reservation);
        Ok(buffer)
    }
}

impl Operator for TopKOperator {
    /// Computes the top rows on the first call (without reading the input when `k = 0`), then
    /// emits them in batches.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        if self.output.is_none() {
            let rows = self.compute(context)?;
            self.output = Some(OutputBuffer::new(rows));
        }
        Ok(self
            .output
            .as_mut()
            .expect("computed above")
            .next_batch(context.batch_size))
    }

    /// `top_k`.
    fn name(&self) -> &'static str {
        "top_k"
    }

    /// The input.
    fn children(&self) -> Vec<&dyn Operator> {
        vec![self.input.as_ref()]
    }

    /// Rows read, compactions and memory.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        let peak = self
            .reservation
            .as_ref()
            .map_or(0, |reservation| reservation.peak() as u64);
        vec![
            ("rows_in", self.rows_in),
            ("compactions", self.compactions),
            ("peak_memory_bytes", peak),
        ]
    }
}
