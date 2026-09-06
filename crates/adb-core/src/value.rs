//! Value module for the adb-core crate.
//!
use serde::{Deserialize, Serialize};

/// Enumerates the supported `Value` variants used by this subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    String(String),
    Bytes(Vec<u8>),
}
