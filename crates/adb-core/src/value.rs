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
