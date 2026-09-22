use std::{collections::BTreeMap, env, fs, hint::black_box, path::PathBuf, time::{Duration, Instant}};

use adb_core::{FieldId, Row, RowId, Value};
use adb_engine::Database;
use adb_execution::{BinaryOp, Expr, PhysicalPlan};
use tempfile::tempdir;

fn row(id: u128) -> Row {
    let mut fields = BTreeMap::new();
    fields.insert(1 as FieldId, Value::Int64(id as i64));
    fields.insert(2 as FieldId, Value::String(format!("row-{id}")));
    Row { fields }
}

fn percentile(ns: &mut [u128], pct: f64) -> u128 {
    ns.sort_unstable();
    if ns.is_empty() { return 0; }
    let idx = (((ns.len() - 1) as f64) * pct).round() as usize;
    ns[idx]
}

fn measure<F: FnMut()>(iters: usize, mut f: F) -> (Duration, u128, u128, u128) {
    let start = Instant::now();
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let s = Instant::now();
        f();
        samples.push(s.elapsed().as_nanos());
    }
    let total = start.elapsed();
    let mut p = samples.clone();
    let p50 = percentile(&mut p, 0.50);
    let p95 = percentile(&mut p, 0.95);
    let p99 = percentile(&mut p, 0.99);
    (total, p50, p95, p99)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rows = 5_000usize;
    let mut iters = 1_000usize;
    let mut output: Option<PathBuf> = None;
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--rows" => { i += 1; rows = args[i].parse()?; }
            "--iters" => { i += 1; iters = args[i].parse()?; }
            "--output" => { i += 1; output = Some(PathBuf::from(&args[i])); }
            other => return Err(format!("unknown argument: {other}").into()),
        }
        i += 1;
    }

    println!("Adaptive DB 2.0.2 Rust benchmark — engine/execution baseline");
    println!("rows={rows} iters={iters}");
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;

    let load_start = Instant::now();
    let batch = 100usize.max(1);
    for base in (0..rows).step_by(batch) {
        let mut tx = db.begin();
        for n in base..(base + batch).min(rows) {
            tx.put(RowId((n + 1) as u128), row((n + 1) as u128));
        }
        db.commit(tx)?;
    }
    let load = load_start.elapsed();

    for _ in 0..100.min(iters) {
        black_box(db.get(RowId(1))?);
    }

    let mut key = 0usize;
    let (point_total, p50, p95, p99) = measure(iters, || {
        key = (key + 7919) % rows.max(1);
        black_box(db.get(RowId((key + 1) as u128)).expect("point lookup"));
    });

    let scan_plan = PhysicalPlan::Scan;
    let scan_start = Instant::now();
    let mut scan_cursor = db.execute(scan_plan)?;
    let mut scan_rows = 0usize;
    while let Some(batch) = scan_cursor.next_batch()? { scan_rows += batch.len(); black_box(batch); }
    let scan = scan_start.elapsed();

    let filter_plan = PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::Filter {
            input: Box::new(PhysicalPlan::Scan),
            predicate: Expr::Binary {
                left: Box::new(Expr::Column { field_id: 1 }),
                op: BinaryOp::Ge,
                right: Box::new(Expr::Literal { value: Value::Int64((rows / 2) as i64) }),
            },
        }),
        limit: 100,
    };
    let plan_json = serde_json::to_string(&filter_plan)?;
    let plan_parse_start = Instant::now();
    for _ in 0..iters { let p: PhysicalPlan = serde_json::from_str(&plan_json)?; black_box(p); }
    let plan_parse = plan_parse_start.elapsed();

    println!("load_ms={} throughput_rows_s={:.0}", load.as_millis(), rows as f64 / load.as_secs_f64());
    println!("point_lookup ops_s={:.0} p50_ns={} p95_ns={} p99_ns={}", iters as f64 / point_total.as_secs_f64(), p50, p95, p99);
    println!("scan rows={} ms={} rows_s={:.0}", scan_rows, scan.as_millis(), scan_rows as f64 / scan.as_secs_f64());
    println!("physical_plan_json_parse ops_s={:.0}", iters as f64 / plan_parse.as_secs_f64());

    if let Some(path) = output {
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
        let json = serde_json::json!({
            "milestone":"2.0.2",
            "layer":"rust",
            "rows":rows,
            "iters":iters,
            "load_ms":load.as_millis(),
            "point_lookup":{"total_ms":point_total.as_millis(),"p50_ns":p50,"p95_ns":p95,"p99_ns":p99},
            "scan":{"rows":scan_rows,"ms":scan.as_millis()},
            "physical_plan_json_parse_ms":plan_parse.as_millis()
        });
        fs::write(path, serde_json::to_vec_pretty(&json)?)?;
    }
    Ok(())
}
