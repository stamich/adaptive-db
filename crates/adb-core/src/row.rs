//! Logical row representation: a sparse map from field id to value.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FieldId, Value};

/// One logical row. Fields are kept ordered so encodings are deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Row {
    /// Field values by field id.
    pub fields: BTreeMap<FieldId, Value>,
}

impl Row {
    /// Creates an empty row.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style setter used mostly by tests and examples.
    pub fn with_field(mut self, field_id: FieldId, value: Value) -> Self {
        self.fields.insert(field_id, value);
        self
    }

    /// Returns the value of `field_id`, if present.
    pub fn get(&self, field_id: FieldId) -> Option<&Value> {
        self.fields.get(&field_id)
    }
}
