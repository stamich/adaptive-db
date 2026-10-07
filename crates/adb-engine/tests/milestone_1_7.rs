//! Physical-plan execution through the engine.
mod common;

use adb_core::{RowId, Value};
use adb_engine::Database;
use adb_execution::{BinaryOp, Expr, PhysicalPlan};
use tempfile::tempdir;

use common::row_with_i64;

/// Scan + filter + limit return exactly the limited rows in one batch.
#[test]
fn database_executes_physical_plan_in_batches() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();

    for id in 1..=100u128 {
        let mut tx = db.begin();
        tx.put(RowId(id), row_with_i64(id as i64));
        db.commit(tx).unwrap();
    }

    let plan = PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::Filter {
            input: Box::new(PhysicalPlan::Scan),
            predicate: Expr::Binary {
                left: Box::new(Expr::Column { field_id: 1 }),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Literal {
                    value: Value::Int64(90),
                }),
            },
        }),
        limit: 5,
    };

    let mut cursor = db.execute(plan).unwrap();
    let batch = cursor.next_batch().unwrap().unwrap();
    assert_eq!(batch.len(), 5);
    assert!(cursor.next_batch().unwrap().is_none());
}
