use adb_core::{Row, Value};

pub fn row_with_i64(value: i64) -> Row {
    Row::new().with_field(1, Value::Int64(value))
}

pub fn read_i64(row: &Row) -> i64 {
    match row.get(1) {
        Some(Value::Int64(v)) => *v,
        other => panic!("expected Int64, got {other:?}"),
    }
}
