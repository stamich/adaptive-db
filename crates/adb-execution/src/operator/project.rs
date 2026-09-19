//! Module `project` for crate `src`.
use std::collections::BTreeMap;

use adb_core::{FieldId, Row};

use crate::{
    operator::{Operator, RowBatch},
    ExecutionContext, ExecutionError,
};

/// Represents `ProjectOperator` state used by this subsystem.
pub struct ProjectOperator {
    input: Box<dyn Operator>,
    fields: Vec<FieldId>,
}

/// Implements behavior for `ProjectOperator`.
impl ProjectOperator {
    /// Implements the `new` operation used by this subsystem.
    pub fn new(input: Box<dyn Operator>, fields: Vec<FieldId>) -> Self {
        Self { input, fields }
    }
}

/// Implements behavior for `Operator`.
impl Operator for ProjectOperator {
    /// Implements the `next_batch` operation used by this subsystem.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        let Some(batch) = self.input.next_batch(context)? else {
            return Ok(None);
        };

        Ok(Some(
            batch
                .into_iter()
                .map(|(row_id, row)| {
                    let mut fields = BTreeMap::new();
                    for field_id in &self.fields {
                        if let Some(value) = row.get(*field_id) {
                            fields.insert(*field_id, value.clone());
                        }
                    }
                    (row_id, Row { fields })
                })
                .collect(),
        ))
    }
}
