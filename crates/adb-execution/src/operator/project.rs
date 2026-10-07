//! Projection: selects the output slots.
use adb_core::Value;

use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError, SlotId,
};

/// Keeps only `slots` in every row; all other slots become NULL.
///
/// Clearing the dropped slots matters for blocking operators above a projection (Sort, TopK):
/// they only hold the values that will be returned.
pub struct ProjectOperator {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Slots kept in every row.
    slots: Vec<SlotId>,
}

impl ProjectOperator {
    /// Projects `input` onto `slots`.
    pub fn new(input: Box<dyn Operator>, slots: Vec<SlotId>) -> Self {
        Self { input, slots }
    }
}

impl Operator for ProjectOperator {
    /// Moves the projected slots of each input row into a fresh all-NULL row.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        let Some(mut batch) = self.input.next_batch(context)? else {
            return Ok(None);
        };

        for row in &mut batch {
            let mut values = vec![Value::Null; row.values.len()];
            for slot in &self.slots {
                values[slot.index()] =
                    std::mem::replace(&mut row.values[slot.index()], Value::Null);
            }
            row.values = values;
        }
        Ok(Some(batch))
    }
}
