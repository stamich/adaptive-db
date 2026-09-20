use adb_core::{Row, RowId, Value};
use adb_engine::Database;
use adb_execution::{BinaryOp, Expr, PhysicalPlan, BATCH_MAGIC};
use adb_ffi::{
    adb_batch_data, adb_batch_len, adb_batch_release, adb_close, adb_execute_plan_json, adb_open,
    adb_query_close, adb_query_next_batch, AdbStatus,
};
use std::{ptr, slice};
use tempfile::tempdir;

fn row(id: u128) -> Row {
    Row::new()
        .with_field(1, Value::Int64(id as i64))
        .with_field(2, Value::String(format!("row-{id}")))
        .with_field(3, Value::Bool(id % 2 == 0))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Adaptive DB 1.7.2 demo — execution + batch wire + C ABI");
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;

    let mut tx = db.begin();
    for id in 1..=100u128 {
        tx.put(RowId(id), row(id));
    }
    let commit_ts = db.commit(tx)?;
    println!("Committed 100 rows at snapshot {:?}", commit_ts);

    let plan = PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::Project {
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
            fields: vec![1, 2],
        }),
        limit: 5,
    };

    let mut cursor = db.execute(plan.clone())?;
    let batch = cursor.next_batch()?.expect("demo query returns rows");
    println!(
        "Execution pipeline returned {} rows / {} columns",
        batch.len(),
        batch.columns.len()
    );
    println!("Query metrics: {:?}", cursor.metrics());

    let point = PhysicalPlan::PointLookup { row_id: RowId(42) };
    let mut point_cursor = db.execute(point)?;
    let point_batch = point_cursor.next_batch()?.expect("point lookup exists");
    println!("PointLookup(row=42) -> {} row", point_batch.len());

    drop(db);
    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut db_handle = ptr::null_mut();
    assert_eq!(
        adb_open(path.as_ptr(), path.len(), &mut db_handle),
        AdbStatus::Ok
    );
    let json = serde_json::to_vec(&plan)?;
    let mut query = ptr::null_mut();
    assert_eq!(
        adb_execute_plan_json(db_handle, json.as_ptr(), json.len(), &mut query),
        AdbStatus::Ok
    );
    let mut ffi_batch = ptr::null_mut();
    assert_eq!(adb_query_next_batch(query, &mut ffi_batch), AdbStatus::Ok);
    let len = adb_batch_len(ffi_batch);
    let bytes = unsafe { slice::from_raw_parts(adb_batch_data(ffi_batch), len) };
    let magic = u32::from_le_bytes(bytes[0..4].try_into()?);
    println!("FFI batch: {} bytes, magic=0x{magic:08x}", len);
    assert_eq!(magic, BATCH_MAGIC);
    assert_eq!(adb_batch_release(ffi_batch), AdbStatus::Ok);
    assert_eq!(adb_query_close(query), AdbStatus::Ok);
    assert_eq!(adb_close(db_handle), AdbStatus::Ok);

    let db = Database::open(dir.path())?;
    println!("Restart PointLookup(42): {:?}", db.get(RowId(42))?);
    println!("Integrity: {:?}", db.verify()?);
    println!("Storage stats: {:?}", db.storage_stats()?);
    println!("Demo completed.");
    Ok(())
}
