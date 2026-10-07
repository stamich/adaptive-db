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
}

impl SortOperator {
    /// Sorts `input` by `keys`.
    pub fn new(input: Box<dyn Operator>, keys: Vec<SortKey>) -> Self {
        Self {
            input,
            keys,
            sorted: None,
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
            self.sorted = Some((OutputBuffer::new(rows), reservation));
        }
        let (buffer, _) = self.sorted.as_mut().expect("sorted above");
        Ok(buffer.next_batch(context.batch_size))
    }
}
