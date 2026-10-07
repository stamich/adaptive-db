//! Relational execution demo of Milestone 2.1.3, driven directly from Rust (no JVM).
//!
//! ```text
//! EntityScan(orders  -> slots 0..2) \
//!                                    HashJoin(o.customer_id = c.id)
//! EntityScan(customer -> slots 3..4) /        |
//!                                    Aggregate(SUM(amount) per city)
//!                                             |
//!                                    TopK(total DESC, 2)
//! ```
//!
//! The plan makes the trip the JVM planner's plans make: it is encoded as plan wire v2 JSON,
//! decoded and validated, executed against a real (temporary) database, and the per-operator
//! runtime profile is printed afterwards.
use adb_core::{Row, RowId, Value};
use adb_engine::Database;
use adb_execution::{
    AggregateFunction, AggregateSpec, JoinKey, JoinType, PhysicalPlan, ScanColumn, SlotId, SortKey,
};

/// Result type of the demo (any error aborts it).
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Entity id of the customer table.
const CUSTOMER: u64 = 1;
/// Entity id of the orders table.
const ORDERS: u64 = 2;

/// Shorthand for a scan column mapping.
fn col(field_id: u32, slot: u32) -> ScanColumn {
    ScanColumn {
        field_id,
        slot: SlotId(slot),
    }
}

/// Loads customers (id f1, name f2, city f3) and orders (id f1, customer_id f2, amount f3).
fn load(db: &Database) -> Result<()> {
    let mut tx = db.begin();
    for (id, name, city) in [
        (1, "Ada", "Krakow"),
        (2, "Ben", "Gdansk"),
        (3, "Cy", "Krakow"),
    ] {
        tx.put(
            RowId::compose(CUSTOMER, id),
            Row::new()
                .with_field(1, Value::Int64(id as i64))
                .with_field(2, Value::String(name.into()))
                .with_field(3, Value::String(city.into())),
        );
    }
    for (id, customer, amount) in [(10u64, 1, 120), (11, 1, 80), (12, 2, 300), (13, 3, 500)] {
        tx.put(
            RowId::compose(ORDERS, id),
            Row::new()
                .with_field(1, Value::Int64(id as i64))
                .with_field(2, Value::Int64(customer))
                .with_field(3, Value::Int64(amount)),
        );
    }
    db.commit(tx)?;
    Ok(())
}

/// `SELECT c.city, SUM(o.amount) AS total FROM orders o JOIN customer c ON o.customer_id = c.id
///  GROUP BY c.city ORDER BY total DESC LIMIT 2` as a physical plan.
fn plan() -> PhysicalPlan {
    PhysicalPlan::TopK {
        input: Box::new(PhysicalPlan::Aggregate {
            input: Box::new(PhysicalPlan::HashJoin {
                left: Box::new(PhysicalPlan::EntityScan {
                    entity_id: ORDERS,
                    columns: vec![col(2, 0), col(3, 1)],
                }),
                right: Box::new(PhysicalPlan::EntityScan {
                    entity_id: CUSTOMER,
                    columns: vec![col(1, 2), col(3, 3)],
                }),
                join_type: JoinType::Inner,
                keys: vec![JoinKey {
                    left: SlotId(0),
                    right: SlotId(2),
                }],
                residual: None,
            }),
            group_by: vec![SlotId(3)],
            aggregates: vec![AggregateSpec {
                function: AggregateFunction::Sum,
                input: Some(SlotId(1)),
                output: SlotId(4),
            }],
        }),
        keys: vec![SortKey {
            slot: SlotId(4),
            descending: true,
        }],
        limit: 2,
    }
}

/// Plain rendering of a value for the result table.
fn show(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(v) => v.to_string(),
        Value::Int64(v) => v.to_string(),
        Value::Float64(v) => v.to_string(),
        Value::String(v) => v.clone(),
        Value::Bytes(v) => format!("<{} bytes>", v.len()),
    }
}

/// Builds, ships, executes and profiles the demo plan.
fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = Database::open(dir.path())?;
    load(&db)?;

    let wire = adb_plan_wire::encode_json(&plan())?;
    println!(
        "plan wire v2 ({} bytes):\n{}\n",
        wire.len(),
        String::from_utf8_lossy(&wire)
    );
    let (decoded, shape) = adb_plan_wire::decode_json(&wire)?;
    println!(
        "decoded and validated: row width {} slots, output slots {:?}\n",
        shape.width, shape.output
    );

    let mut cursor = db.execute(decoded)?;
    println!("city   | total");
    while let Some(batch) = cursor.next_batch()? {
        for row in 0..batch.len() {
            println!(
                "{:6} | {}",
                show(&batch.value(row, SlotId(3))),
                show(&batch.value(row, SlotId(4)))
            );
        }
    }
    println!(
        "\nruntime profile:\n{}",
        serde_json::to_string_pretty(&cursor.profile())?
    );
    Ok(())
}
