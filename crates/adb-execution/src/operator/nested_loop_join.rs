//! Nested-loop join (INNER / LEFT) for arbitrary join conditions.
use crate::{
    memory::MemoryReservation,
    operator::{
        collect::collect_all_bounded,
        join::{check_fanout, combine, fill_batch, finish_left_row, LeftStream},
        Operator, RowBatch,
    },
    ExecRow, ExecutionContext, ExecutionError, Expr, JoinType, SlotId,
};

/// Materializes the right input, then evaluates the predicate for every (left, right) pair.
///
/// Cost is O(|left| x |right|), so the total number of evaluated pairs is capped by
/// [`crate::ExecutionLimits::max_nested_loop_comparisons`]; the planner only chooses this
/// operator when no equality key exists.
pub struct NestedLoopJoinOperator {
    /// Outer side.
    left: LeftStream,
    /// Inner side input, consumed by the first pull.
    right: Box<dyn Operator>,
    /// Inner rows and their memory, once materialized.
    inner: Option<(Vec<ExecRow>, MemoryReservation)>,
    /// INNER or LEFT.
    join_type: JoinType,
    /// Join condition; `None` matches every pair.
    predicate: Option<Expr>,
    /// Slots produced by the right input.
    right_slots: Vec<SlotId>,
    /// Pairs evaluated so far.
    comparisons: u64,
}

impl NestedLoopJoinOperator {
    /// Joins `left` and `right` on `predicate`.
    pub fn new(
        left: Box<dyn Operator>,
        right: Box<dyn Operator>,
        join_type: JoinType,
        predicate: Option<Expr>,
        right_slots: Vec<SlotId>,
    ) -> Self {
        Self {
            left: LeftStream::new(left),
            right,
            inner: None,
            join_type,
            predicate,
            right_slots,
            comparisons: 0,
        }
    }
}

impl Operator for NestedLoopJoinOperator {
    /// Materializes the inner side on the first call, then joins left rows until a batch is full.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        if self.inner.is_none() {
            let mut reservation = context.memory.reservation("nested loop join inner side");
            let rows = collect_all_bounded(
                self.right.as_mut(),
                context,
                &mut reservation,
                "nested loop join inner side",
            )?;
            self.inner = Some((rows, reservation));
        }
        let Self {
            left,
            inner,
            join_type,
            predicate,
            right_slots,
            comparisons,
            ..
        } = self;
        let (inner_rows, _) = inner.as_ref().expect("materialized above");
        let max_comparisons = context.limits.max_nested_loop_comparisons;

        fill_batch(left, context, |left_row, out| {
            let mut matches = 0;
            for right_row in inner_rows {
                *comparisons += 1;
                if *comparisons > max_comparisons {
                    return Err(ExecutionError::ResourceLimit(format!(
                        "nested loop join exceeds {max_comparisons} comparisons"
                    )));
                }
                let joined = combine(&left_row, right_row, right_slots);
                if let Some(predicate) = predicate {
                    if !predicate.evaluate_bool(&joined)? {
                        continue;
                    }
                }
                matches += 1;
                check_fanout(matches, context)?;
                out.push(joined);
            }
            finish_left_row(left_row, matches, *join_type, right_slots, out);
            Ok(())
        })
    }
}
