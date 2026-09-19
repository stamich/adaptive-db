//! Module `value` for crate `adb-core`.
use serde::{Deserialize, Serialize};

/// Enumerates `Value` alternatives used by this subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    String(String),
    Bytes(Vec<u8>),
}
