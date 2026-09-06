//! Common module for the adb-engine crate.
//!
use adb_core::{Row, Value};

/// Implements the `row_with_i64` operation used by this subsystem.
pub fn row_with_i64(value: i64) -> Row {
    Row::new().with_field(1, Value::Int64(value))
}

/// Reads the `i64` value from the binary representation.
pub fn read_i64(row: &Row) -> i64 {
    match row.get(1) {
        Some(Value::Int64(v)) => *v,
        other => panic!("expected Int64, got {other:?}"),
    }
}
