//! Scalar values stored in rows.
use serde::{Deserialize, Serialize};

/// A dynamically typed scalar. The JSON form (`{"Int64": 5}`) is part of the C ABI contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// SQL NULL.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed 64-bit integer.
    Int64(i64),
    /// IEEE-754 double.
    Float64(f64),
    /// UTF-8 string.
    String(String),
    /// Opaque bytes.
    Bytes(Vec<u8>),
}

/// The type of a non-NULL [`Value`]: the physical column types a schema declares.
///
/// The JSON form (`"Int64"`) is part of the C ABI schema documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValueType {
    /// [`Value::Bool`].
    Bool,
    /// [`Value::Int64`].
    Int64,
    /// [`Value::Float64`].
    Float64,
    /// [`Value::String`].
    String,
    /// [`Value::Bytes`].
    Bytes,
}

impl ValueType {
    /// The type of `value`; `None` for NULL, which fits every nullable column.
    pub fn of(value: &Value) -> Option<ValueType> {
        match value {
            Value::Null => None,
            Value::Bool(_) => Some(ValueType::Bool),
            Value::Int64(_) => Some(ValueType::Int64),
            Value::Float64(_) => Some(ValueType::Float64),
            Value::String(_) => Some(ValueType::String),
            Value::Bytes(_) => Some(ValueType::Bytes),
        }
    }

    /// Whether `value` is NULL or of this type.
    pub fn admits(self, value: &Value) -> bool {
        ValueType::of(value).is_none_or(|found| found == self)
    }
}
