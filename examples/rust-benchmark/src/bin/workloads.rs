//! Milestone 2.2.3 workload benchmark, `rust_native` path.
//!
//! Loads the shared benchmark tables with the formulas the JVM benchmark uses
//! (`bench_customer`: 100 rows, `bench_orders`: 1,000 rows), runs `ANALYZE`, and executes the
//! physical plans the cost-based optimizer produces for workloads A and B directly in Rust,
//! without SQL, planning or FFI. Comparing its timings with the JVM's `ffi_prepared_plan` and
//! `scala_cbo_ffi_rust` paths isolates the cost of the boundary and of planning.
//!
//! ```text
//! cargo run --release -p adb-benchmark-rust --bin workloads -- --iters 100 --output examples/results/2.2.3-database.json
//! ```

use std::{env, path::PathBuf, time::Instant};

use adb_core::{Row, RowId, Value};
use adb_engine::{AnalyzeOptions, Database};
use adb_execution::{
    AggregateFunction, AggregateSpec, ColumnVector, JoinKey, JoinType, PhysicalPlan, ScanColumn,
    SlotId, SortKey,
};
use serde_json::json;
use tempfile::tempdir;

/// Result type of the benchmark (any error aborts it).
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Entity id of `bench_customer` (the first table a fresh JVM catalog creates).
const CUSTOMER: u64 = 1;
/// Entity id of `bench_orders`.
const ORDERS: u64 = 2;
/// Rows of `bench_customer`.
const CUSTOMERS: u64 = 100;
/// Rows of `bench_orders`.
const ORDERS_ROWS: u64 = 1000;

/// `bench_customer` row `id`: (1 id, 2 name = `customer-<id>`, 3 segment = id % 5).
fn customer(id: u64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(id as i64))
        .with_field(2, Value::String(format!("customer-{id}")))
        .with_field(3, Value::Int64((id % 5) as i64))
}

/// `bench_orders` row `id`: (1 id, 2 customer_id = 1 + 7·id mod 100, 3 amount = 37·id mod 1000 + 1).
fn order(id: u64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(id as i64))
        .with_field(2, Value::Int64((1 + (id * 7) % CUSTOMERS) as i64))
        .with_field(3, Value::Int64(((id * 37) % 1000 + 1) as i64))
}

/// Shorthand for a scan column mapping.
fn col(field_id: u32, slot: u32) -> ScanColumn {
    ScanColumn {
        field_id,
        slot: SlotId(slot),
    }
}

/// `bench_orders o JOIN bench_customer c ON c.id = o.customer_id`, building the smaller
/// customer side (the cost-based plan): slots 0 = c.id, 1 = o.customer_id, 2 = c.name, 3 = o.amount.
fn join() -> PhysicalPlan {
    PhysicalPlan::HashJoin {
        left: Box::new(PhysicalPlan::EntityScan {
            entity_id: ORDERS,
            columns: vec![col(2, 1), col(3, 3)],
        }),
        right: Box::new(PhysicalPlan::EntityScan {
            entity_id: CUSTOMER,
            columns: vec![col(1, 0), col(2, 2)],
        }),
        join_type: JoinType::Inner,
        keys: vec![JoinKey {
            left: SlotId(1),
            right: SlotId(0),
        }],
        residual: None,
    }
}

/// Workload A: `SELECT c.name, o.amount ... ORDER BY o.amount DESC LIMIT 10`.
fn workload_a() -> PhysicalPlan {
    PhysicalPlan::Project {
        input: Box::new(PhysicalPlan::TopK {
            input: Box::new(join()),
            keys: vec![SortKey {
                slot: SlotId(3),
                descending: true,
            }],
            limit: 10,
        }),
        slots: vec![SlotId(2), SlotId(3)],
    }
}

/// Workload B: `SELECT c.name, SUM(o.amount) AS total ... GROUP BY c.name ORDER BY total DESC LIMIT 10`.
fn workload_b() -> PhysicalPlan {
    PhysicalPlan::Project {
        input: Box::new(PhysicalPlan::TopK {
            input: Box::new(PhysicalPlan::Aggregate {
                input: Box::new(join()),
                group_by: vec![SlotId(2)],
                aggregates: vec![AggregateSpec {
                    function: AggregateFunction::Sum,
                    input: Some(SlotId(3)),
                    output: SlotId(4),
                }],
            }),
            keys: vec![SortKey {
                slot: SlotId(4),
                descending: true,
            }],
            limit: 10,
        }),
        slots: vec![SlotId(2), SlotId(4)],
    }
}

/// Executes `plan`; returns the rows and the sum of every INT64 output value (a checksum the
/// JVM benchmark computes the same way).
fn run(db: &Database, plan: &PhysicalPlan) -> Result<(usize, i64)> {
    let mut cursor = db.execute(plan.clone())?;
    let (mut rows, mut checksum) = (0, 0i64);
    while let Some(batch) = cursor.next_batch()? {
        rows += batch.len();
        for column in &batch.columns {
            if let ColumnVector::Int64 { values, .. } = column {
                checksum += values.iter().flatten().sum::<i64>();
            }
        }
    }
    Ok((rows, checksum))
}

/// `iters` timed executions after 10 warm-up runs: p50, p95 and mean milliseconds.
fn measure(db: &Database, plan: &PhysicalPlan, iters: usize) -> Result<serde_json::Value> {
    for _ in 0..10 {
        run(db, plan)?;
    }
    let mut samples = Vec::with_capacity(iters);
    let mut result = (0, 0);
    for _ in 0..iters {
        let start = Instant::now();
        result = run(db, plan)?;
        samples.push(start.elapsed().as_secs_f64() * 1e3);
    }
    samples.sort_by(f64::total_cmp);
    let percentile = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    Ok(json!({
        "iterations": iters,
        "p50_ms": percentile(0.5),
        "p95_ms": percentile(0.95),
        "mean_ms": samples.iter().sum::<f64>() / samples.len() as f64,
        "rows_out": result.0,
        "checksum": result.1,
    }))
}

/// Parses `--iters N` and `--output PATH`.
fn parse_args() -> Result<(usize, Option<PathBuf>)> {
    let (mut iters, mut output) = (100, None);
    let raw: Vec<String> = env::args().skip(1).collect();
    for pair in raw.chunks(2) {
        match (pair[0].as_str(), pair.get(1)) {
            ("--iters", Some(value)) => iters = value.parse()?,
            ("--output", Some(value)) => output = Some(PathBuf::from(value)),
            (other, _) => return Err(format!("unknown or incomplete argument {other}").into()),
        }
    }
    Ok((iters.max(1), output))
}

/// Loads, analyzes, measures and reports.
fn main() -> Result<()> {
    let (iters, output) = parse_args()?;
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let mut tx = db.begin();
    for id in 1..=CUSTOMERS {
        tx.put(RowId::compose(CUSTOMER, id), customer(id));
    }
    for id in 1..=ORDERS_ROWS {
        tx.put(RowId::compose(ORDERS, id), order(id));
    }
    db.commit(tx)?;

    let start = Instant::now();
    let customers = db.analyze(CUSTOMER, &AnalyzeOptions::default())?;
    let orders = db.analyze(ORDERS, &AnalyzeOptions::default())?;
    let analyze_ms = start.elapsed().as_secs_f64() * 1e3;

    // ANALYZE at a larger size: 100,000 rows with 1,000 distinct values per column.
    let large = 3;
    for base in (0..100_000u64).step_by(5_000) {
        let mut tx = db.begin();
        for id in base..base + 5_000 {
            tx.put(
                RowId::compose(large, id),
                Row::new()
                    .with_field(1, Value::Int64(id as i64))
                    .with_field(2, Value::Int64((id % 1000) as i64))
                    .with_field(3, Value::String(format!("tag-{}", id % 50))),
            );
        }
        db.commit(tx)?;
    }
    let start = Instant::now();
    let large_stats = db.analyze(large, &AnalyzeOptions::default())?;
    let analyze_large_ms = start.elapsed().as_secs_f64() * 1e3;

    let report = json!({
        "milestone": "2.2.3",
        "path": "rust_native",
        "host": {"os": env::consts::OS, "arch": env::consts::ARCH, "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), "build": if cfg!(debug_assertions) { "debug" } else { "release" }},
        "setup": {"bench_customer": CUSTOMERS, "bench_orders": ORDERS_ROWS, "iterations": iters},
        "analyze": {
            "bench_tables_ms": analyze_ms,
            "bench_orders_distinct_customer_id": orders.column(2).map(|c| c.distinct_count),
            "bench_customer_rows": customers.row_count,
            "rows_100k_ms": analyze_large_ms,
            // 1,000 distinct values: counted exactly. 100,000 unique ids: HyperLogLog estimate.
            "rows_100k_distinct_of_1000": large_stats.column(2).map(|c| c.distinct_count),
            "rows_100k_distinct_of_100000": large_stats.column(1).map(|c| c.distinct_count),
        },
        "workloads": {
            "A_hash_join_topk": measure(&db, &workload_a(), iters)?,
            "B_hash_join_aggregate_topk": measure(&db, &workload_b(), iters)?,
        },
    });
    let text = serde_json::to_string_pretty(&report)?;
    println!("{text}");
    if let Some(path) = output {
        std::fs::write(path, text + "\n")?;
    }
    Ok(())
}
