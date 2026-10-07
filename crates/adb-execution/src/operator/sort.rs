//! Full sort.
use crate::{
    memory::MemoryReservation,
    operator::{
        collect::collect_all_bounded, ordering::sort_rows, output_buffer::OutputBuffer, Operator,
        RowBatch,
    },
    ExecutionContext, ExecutionError, SortKey,
};

/// Materializes its input (memory-accounted), sorts it stably, then emits it in batches.
pub struct SortOperator {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Sort keys, most significant first.
    keys: Vec<SortKey>,
    /// Sorted rows and their memory, once computed.
    sorted: Option<(OutputBuffer, MemoryReservation)>,
    /// Rows sorted.
    rows: u64,
}

impl SortOperator {
    /// Sorts `input` by `keys`.
    pub fn new(input: Box<dyn Operator>, keys: Vec<SortKey>) -> Self {
        Self {
            input,
            keys,
            sorted: None,
            rows: 0,
        }
    }
}

impl Operator for SortOperator {
    /// Sorts everything on the first call, then emits batches.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        if self.sorted.is_none() {
            let mut reservation = context.memory.reservation("sort");
            let mut rows =
                collect_all_bounded(self.input.as_mut(), context, &mut reservation, "sort")?;
            context.check_running()?;
            sort_rows(&mut rows, &self.keys)?;
            self.rows = rows.len() as u64;
            self.sorted = Some((OutputBuffer::new(rows), reservation));
        }
        let (buffer, _) = self.sorted.as_mut().expect("sorted above");
        Ok(buffer.next_batch(context.batch_size))
    }

    /// `sort`.
    fn name(&self) -> &'static str {
        "sort"
    }

    /// The input.
    fn children(&self) -> Vec<&dyn Operator> {
        vec![self.input.as_ref()]
    }

    /// Rows sorted and memory.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        let peak = self
            .sorted
            .as_ref()
            .map_or(0, |(_, reservation)| reservation.peak() as u64);
        vec![("rows_sorted", self.rows), ("peak_memory_bytes", peak)]
    }
}
