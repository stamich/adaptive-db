//! Milestone 2.2.3 workload benchmark.
//!
//! Two modes:
//!
//! * **Measure** (`rust_native` path): loads the dataset ([`dataset`]) at a scale factor into a
//!   temporary database, times `ANALYZE` of every table, and executes the physical plans the
//!   cost-based optimizer produces for workloads A and B directly in Rust, without SQL,
//!   planning or FFI. Comparing its timings with the JVM's `ffi_prepared_plan` and
//!   `scala_cbo_ffi_rust` paths isolates the cost of the boundary and of planning.
//! * **Prepare** (`--prepare DIR`): loads the dataset into `DIR` and prepares the stale-statistics
//!   table, for the JVM benchmark to open. Loading through SQL would cost one commit per row.
//!
//! ```text
//! cargo run --release -p adb-benchmark-rust --bin workloads -- --scale 10 --iters 100 --output out.json
//! cargo run --release -p adb-benchmark-rust --bin workloads -- --prepare /tmp/bench/rust --scale 10
//! ```

mod dataset;

use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

use adb_engine::{AnalyzeOptions, Database};
use adb_execution::{
    AggregateFunction, AggregateSpec, ColumnVector, JoinKey, JoinType, PhysicalPlan, ScanColumn,
    SlotId, SortKey,
};
use serde_json::json;
use tempfile::tempdir;

/// Result type of the benchmark (any error aborts it).
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
            entity_id: dataset::by_name("bench_orders").entity,
            columns: vec![col(2, 1), col(3, 3)],
        }),
        right: Box::new(PhysicalPlan::EntityScan {
            entity_id: dataset::by_name("bench_customer").entity,
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

/// Outcome of one execution.
struct Run {
    /// Rows returned.
    rows: usize,
    /// Sum of every INT64 output value (the JVM benchmark computes the same checksum).
    checksum: i64,
    /// Peak bytes the query's blocking operators held.
    peak_memory_bytes: u64,
}

/// Executes `plan` once.
fn run(db: &Database, plan: &PhysicalPlan) -> Result<Run> {
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
    Ok(Run {
        rows,
        checksum,
        peak_memory_bytes: cursor.profile().peak_memory_bytes,
    })
}

/// Warms up for at least `warmup` (and 10 runs), then times `iters` executions: p50, p95, p99,
/// mean, input rows per second at the median, rows, checksum and peak memory.
fn measure(
    db: &Database,
    plan: &PhysicalPlan,
    iters: usize,
    warmup: Duration,
    input_rows: u64,
) -> Result<serde_json::Value> {
    let start = Instant::now();
    let mut warm = 0;
    while warm < 10 || start.elapsed() < warmup {
        run(db, plan)?;
        warm += 1;
    }
    let mut samples = Vec::with_capacity(iters);
    let mut last = run(db, plan)?;
    for _ in 0..iters {
        let start = Instant::now();
        last = run(db, plan)?;
        samples.push(start.elapsed().as_secs_f64() * 1e3);
    }
    samples.sort_by(f64::total_cmp);
    let percentile = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    Ok(json!({
        "iterations": iters,
        "warmup_runs": warm,
        "p50_ms": percentile(0.5),
        "p95_ms": percentile(0.95),
        "p99_ms": percentile(0.99),
        "mean_ms": samples.iter().sum::<f64>() / samples.len() as f64,
        "input_rows": input_rows,
        "input_rows_per_s": input_rows as f64 / (percentile(0.5) / 1e3),
        "rows_out": last.rows,
        "checksum": last.checksum,
        "peak_memory_bytes": last.peak_memory_bytes,
    }))
}

/// Command-line options.
struct Args {
    /// Timed executions per workload.
    iters: usize,
    /// Scale factor of the dataset.
    scale: u64,
    /// Minimum warm-up time per workload.
    warmup: Duration,
    /// Report path (measure mode).
    output: Option<PathBuf>,
    /// Directory to prepare (prepare mode).
    prepare: Option<PathBuf>,
}

/// Parses `--iters N`, `--scale N`, `--warmup-ms N`, `--output PATH` and `--prepare DIR`.
fn parse_args() -> Result<Args> {
    let mut args = Args {
        iters: 100,
        scale: 1,
        warmup: Duration::from_millis(200),
        output: None,
        prepare: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    for pair in raw.chunks(2) {
        match (pair[0].as_str(), pair.get(1)) {
            ("--iters", Some(value)) => args.iters = value.parse::<usize>()?.max(1),
            ("--scale", Some(value)) => args.scale = value.parse::<u64>()?.clamp(1, 1_000),
            ("--warmup-ms", Some(value)) => args.warmup = Duration::from_millis(value.parse()?),
            ("--output", Some(value)) => args.output = Some(PathBuf::from(value)),
            ("--prepare", Some(value)) => args.prepare = Some(PathBuf::from(value)),
            (other, _) => return Err(format!("unknown or incomplete argument {other}").into()),
        }
    }
    Ok(args)
}

/// CPU model from `/proc/cpuinfo` (Linux), or "unknown".
fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("model name"))
                .and_then(|line| line.split(':').nth(1))
                .map(|model| model.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// Loads the dataset into `dir` for the JVM benchmark.
fn prepare(dir: &PathBuf, scale: u64) -> Result<()> {
    if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
        return Err(format!("{} must be empty or absent", dir.display()).into());
    }
    let db = Database::open(dir)?;
    let (rows, seconds) = dataset::load(&db, scale)?;
    dataset::make_stale(&db, scale)?;
    db.close()?;
    println!(
        "prepared {rows} rows at scale {scale} in {seconds:.1} s ({:.0} rows/s) into {}",
        rows as f64 / seconds,
        dir.display()
    );
    Ok(())
}

/// Loads, analyzes, measures and reports (or prepares a directory).
fn main() -> Result<()> {
    let args = parse_args()?;
    if let Some(dir) = &args.prepare {
        return prepare(dir, args.scale);
    }
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let (loaded, load_seconds) = dataset::load(&db, args.scale)?;

    let mut analyze = serde_json::Map::new();
    let start = Instant::now();
    for table in &dataset::TABLES {
        let one = Instant::now();
        let statistics = db.analyze(table.entity, &AnalyzeOptions::default())?;
        analyze.insert(
            table.name.into(),
            json!({
                "rows": statistics.row_count,
                "ms": one.elapsed().as_secs_f64() * 1e3,
                "exact": statistics.exact,
            }),
        );
    }
    let analyze_seconds = start.elapsed().as_secs_f64();
    analyze.insert(
        "total".into(),
        json!({"rows": loaded, "ms": analyze_seconds * 1e3, "rows_per_s": loaded as f64 / analyze_seconds}),
    );
    // HyperLogLog check: sk_events ids are unique, so the distinct count should equal the rows.
    let events = dataset::by_name("sk_events");
    let ids = db
        .statistics(events.entity)
        .and_then(|s| s.column(1).map(|c| c.distinct_count));
    analyze.insert(
        "sk_events_distinct_ids".into(),
        json!({"estimated": ids, "actual": events.rows(args.scale)}),
    );

    let join_rows =
        dataset::rows("bench_customer", args.scale) + dataset::rows("bench_orders", args.scale);
    let report = json!({
        "milestone": "2.2.3",
        "path": "rust_native",
        "host": {
            "os": env::consts::OS,
            "arch": env::consts::ARCH,
            "cpu": cpu_model(),
            "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        },
        "setup": {
            "scale": args.scale,
            "bench_customer": dataset::rows("bench_customer", args.scale),
            "bench_orders": dataset::rows("bench_orders", args.scale),
            "iterations": args.iters,
            "warmup_ms": args.warmup.as_millis() as u64,
        },
        "load": {"rows": loaded, "seconds": load_seconds, "rows_per_s": loaded as f64 / load_seconds},
        "analyze": analyze,
        "workloads": {
            "A_hash_join_topk": measure(&db, &workload_a(), args.iters, args.warmup, join_rows)?,
            "B_hash_join_aggregate_topk": measure(&db, &workload_b(), args.iters, args.warmup, join_rows)?,
        },
    });
    let text = serde_json::to_string_pretty(&report)?;
    println!("{text}");
    if let Some(path) = args.output {
        std::fs::write(path, text + "\n")?;
    }
    Ok(())
}
