//! Module `row` for crate `adb-core`.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FieldId, Value};

/// Represents `Row` state used by this subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Row {
    pub fields: BTreeMap<FieldId, Value>,
}

/// Implements behavior for `Row`.
impl Row {
    /// Implements the `new` operation used by this subsystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Implements the `with_field` operation used by this subsystem.
    pub fn with_field(mut self, field_id: FieldId, value: Value) -> Self {
        self.fields.insert(field_id, value);
        self
    }

    /// Implements the `get` operation used by this subsystem.
    pub fn get(&self, field_id: FieldId) -> Option<&Value> {
        self.fields.get(&field_id)
    }
}
