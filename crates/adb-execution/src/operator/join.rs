//! Pieces shared by the join operators: the streamed left side and output-row construction.
use std::collections::VecDeque;

use adb_core::Value;

use crate::{operator::Operator, ExecRow, ExecutionContext, ExecutionError, JoinType, SlotId};

/// The streamed (left/probe/outer) input, consumed one row at a time.
pub struct LeftStream {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Rows of the current input batch not yet joined.
    pending: VecDeque<ExecRow>,
    /// Whether the input is exhausted.
    done: bool,
    /// Rows pulled from the input.
    rows: u64,
}

impl LeftStream {
    /// Streams `input`.
    pub fn new(input: Box<dyn Operator>) -> Self {
        Self {
            input,
            pending: VecDeque::new(),
            done: false,
            rows: 0,
        }
    }

    /// Next left row, pulling a new input batch when the current one is used up.
    pub fn next_row(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<ExecRow>, ExecutionError> {
        while self.pending.is_empty() && !self.done {
            context.check_running()?;
            match self.input.next_batch(context)? {
                Some(batch) => {
                    self.rows += batch.len() as u64;
                    self.pending.extend(batch);
                }
                None => self.done = true,
            }
        }
        Ok(self.pending.pop_front())
    }

    /// The upstream operator (for profiling).
    pub fn input(&self) -> &dyn Operator {
        self.input.as_ref()
    }

    /// Rows pulled from the input so far.
    pub fn rows(&self) -> u64 {
        self.rows
    }
}

/// Joined row: `left` with the `right_slots` of `right` copied in. Derived rows have no storage key.
pub fn combine(left: &ExecRow, right: &ExecRow, right_slots: &[SlotId]) -> ExecRow {
    let mut row = left.clone();
    row.row_id = None;
    row.copy_slots_from(right, right_slots);
    row
}

/// LEFT JOIN output for an unmatched left row: every right slot NULL.
pub fn null_extended(mut left: ExecRow, right_slots: &[SlotId]) -> ExecRow {
    left.row_id = None;
    for slot in right_slots {
        left.set(*slot, Value::Null);
    }
    left
}

/// Applies the per-left-row output rules shared by both joins: the fanout cap and LEFT JOIN
/// null-filling. `matches` is how many rows the left row produced.
pub fn finish_left_row(
    left: ExecRow,
    matches: usize,
    join_type: JoinType,
    right_slots: &[SlotId],
    out: &mut Vec<ExecRow>,
) {
    if matches == 0 && join_type == JoinType::Left {
        out.push(null_extended(left, right_slots));
    }
}

/// Fails once one left row exceeds the configured fanout.
pub fn check_fanout(matches: usize, context: &ExecutionContext) -> Result<(), ExecutionError> {
    let limit = context.limits.max_join_fanout;
    if matches > limit {
        return Err(ExecutionError::ResourceLimit(format!(
            "join fanout exceeds {limit} matches for one left row"
        )));
    }
    Ok(())
}

/// Collects joined rows until the batch is full or the left side ends.
///
/// A batch holds at most `batch_size + max_join_fanout` rows: the last left row may overshoot
/// the batch size by its matches, which the fanout cap bounds.
pub fn fill_batch(
    left: &mut LeftStream,
    context: &ExecutionContext,
    mut emit: impl FnMut(ExecRow, &mut Vec<ExecRow>) -> Result<(), ExecutionError>,
) -> Result<Option<Vec<ExecRow>>, ExecutionError> {
    let target = context.batch_size.max(1);
    let mut out = Vec::new();
    while out.len() < target {
        let Some(row) = left.next_row(context)? else {
            break;
        };
        emit(row, &mut out)?;
    }
    Ok((!out.is_empty()).then_some(out))
}
