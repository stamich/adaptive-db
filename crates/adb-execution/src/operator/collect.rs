//! Shared input materialization of the blocking operators.
use crate::{
    memory::MemoryReservation, operator::Operator, ExecRow, ExecutionContext, ExecutionError,
};

/// Pulls `input` to exhaustion and returns every row.
///
/// Each row is reserved in `reservation` before it is kept, and the row count is capped by
/// [`crate::limits::ExecutionLimits::max_materialized_rows`], so a blocking operator fails
/// with `ResourceLimit` instead of exhausting process memory. Cancellation and deadlines are
/// checked between input batches.
pub fn collect_all_bounded(
    input: &mut dyn Operator,
    context: &ExecutionContext,
    reservation: &mut MemoryReservation,
    owner: &str,
) -> Result<Vec<ExecRow>, ExecutionError> {
    let max_rows = context.limits.max_materialized_rows;
    let mut rows = Vec::new();
    while let Some(batch) = input.next_batch(context)? {
        context.check_running()?;
        if rows.len() + batch.len() > max_rows {
            return Err(ExecutionError::ResourceLimit(format!(
                "{owner} would materialize more than {max_rows} rows"
            )));
        }
        let bytes = batch.iter().map(ExecRow::estimated_bytes).sum();
        reservation.grow(bytes)?;
        rows.extend(batch);
    }
    Ok(rows)
}
