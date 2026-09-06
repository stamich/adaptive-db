use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FieldId, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Row {
    pub fields: BTreeMap<FieldId, Value>,
}

impl Row {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_field(mut self, field_id: FieldId, value: Value) -> Self {
        self.fields.insert(field_id, value);
        self
    }

    pub fn get(&self, field_id: FieldId) -> Option<&Value> {
        self.fields.get(&field_id)
    }
}
