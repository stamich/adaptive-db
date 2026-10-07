//! Helpers shared by the engine integration tests.
#![allow(dead_code)]

use adb_core::{CommitTs, Row, RowId, Value};
use adb_engine::Database;

/// A row with field 1 = `value`.
pub fn row_with_i64(value: i64) -> Row {
    Row::new().with_field(1, Value::Int64(value))
}

/// Field 1 of `row`.
pub fn read_i64(row: &Row) -> i64 {
    match row.get(1) {
        Some(Value::Int64(v)) => *v,
        other => panic!("expected Int64, got {other:?}"),
    }
}

/// Commits one put in its own transaction.
pub fn put(db: &Database, row_id: RowId, value: i64) -> CommitTs {
    let mut tx = db.begin();
    tx.put(row_id, row_with_i64(value));
    db.commit(tx).unwrap()
}

/// Commits one delete in its own transaction.
pub fn delete(db: &Database, row_id: RowId) -> CommitTs {
    let mut tx = db.begin();
    tx.delete(row_id);
    db.commit(tx).unwrap()
}

/// Field-1 values of the latest committed state of `row_id`.
pub fn value(db: &Database, row_id: RowId) -> Option<i64> {
    db.get(row_id).unwrap().map(|row| read_i64(&row))
}
