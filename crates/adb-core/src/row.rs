//! Row module for the adb-core crate.
//!
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FieldId, Value};

/// Represents `Row` state used by the src subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Row {
    pub fields: BTreeMap<FieldId, Value>,
}

/// Implements behavior for `Row`.
impl Row {
    /// Creates a new instance initialized with the supplied state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Implements the `with_field` operation used by this subsystem.
    pub fn with_field(mut self, field_id: FieldId, value: Value) -> Self {
        self.fields.insert(field_id, value);
        self
    }

    /// Returns the value visible for the requested key or row at the operation's default snapshot.
    pub fn get(&self, field_id: FieldId) -> Option<&Value> {
        self.fields.get(&field_id)
    }
}
