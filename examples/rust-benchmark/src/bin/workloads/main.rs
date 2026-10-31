//! Milestone 2.4.0 workload benchmark.
//!
//! Two modes:
//!
//! * **Measure** (`rust_native` path): loads the dataset ([`dataset`]) at a scale factor into a
//!   temporary database, times `ANALYZE` of every table and the build of every index, and
//!   executes the physical plans the cost-based optimizer produces for workloads A–E directly
//!   in Rust, without SQL, planning or FFI. Comparing its timings with the JVM's
//!   `ffi_prepared_plan` and `scala_cbo_ffi_rust` paths isolates the cost of the boundary and
//!   of planning. It also measures the write overhead of 0, 1 and 3 indexes and an index read
//!   at an older (`AS OF`) snapshot. Since 2.4.0 it also runs the schema-evolution workloads
//!   F–H, ALTER latency and write validation ([`schema`]) on a database of their own.
//! * **Prepare** (`--prepare DIR`): loads the dataset into `DIR` and prepares the stale-statistics
//!   table, for the JVM benchmark to open. Loading through SQL would cost one commit per row.
//!
//! ```text
//! cargo run --release -p adb-benchmark-rust --bin workloads -- --scale 10 --iters 100 --output out.json
//! cargo run --release -p adb-benchmark-rust --bin workloads -- --prepare /tmp/bench/rust --scale 10
//! ```

mod dataset;
mod schema;

use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

use adb_core::{CommitTs, Direction, Row, RowId, Value};
use adb_engine::{AnalyzeOptions, Database, IndexColumn, IndexSpec};
use adb_execution::{
    AggregateFunction, AggregateSpec, BinaryOp, ColumnVector, Expr, JoinKey, JoinType,
    PhysicalPlan, ScanColumn, SlotId, SortKey,
};
use adb_index::{IndexBounds, ScanDirection, ValueBound};
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

/// Entity id of `ix_account`.
fn ix_account() -> u64 {
    dataset::by_name("ix_account").entity
}

/// Workload C: `SELECT owner, balance FROM ix_account WHERE owner = 'owner-42'`, a covering
/// equality scan of `idx_ix_owner` (slots 0 = owner, 1 = balance).
fn workload_c(owner_index: u64) -> PhysicalPlan {
    PhysicalPlan::IndexScan {
        index_id: owner_index,
        bounds: IndexBounds {
            prefix: vec![Value::String("owner-42".into())],
            ..IndexBounds::all()
        },
        direction: ScanDirection::Forward,
        columns: vec![col(4, 0), col(3, 1)],
        limit: None,
    }
}

/// Lower balance bound of workload D: the first 1% of the balance range.
const D_FROM: i64 = (dataset::BALANCE_MODULUS / 100) as i64;

/// Workload D: `SELECT id, balance FROM ix_account WHERE balance >= 10000000 ORDER BY balance
/// LIMIT 100`, an ordered range scan of `idx_ix_balance` with a row fetch and no sort
/// (slots 0 = balance, 1 = id).
fn workload_d(balance_index: u64) -> PhysicalPlan {
    PhysicalPlan::Project {
        input: Box::new(PhysicalPlan::Limit {
            input: Box::new(PhysicalPlan::RowFetch {
                input: Box::new(PhysicalPlan::IndexScan {
                    index_id: balance_index,
                    bounds: IndexBounds {
                        prefix: vec![],
                        lower: Some(ValueBound {
                            value: Value::Int64(D_FROM),
                            inclusive: true,
                        }),
                        upper: None,
                    },
                    direction: ScanDirection::Forward,
                    columns: vec![col(3, 0)],
                    limit: Some(100),
                }),
                columns: vec![col(1, 1)],
            }),
            limit: 100,
        }),
        slots: vec![SlotId(1), SlotId(0)],
    }
}

/// Workload E (control, no index on `region`): `SELECT COUNT(*) FROM ix_account WHERE
/// region = 7`, a sequential scan with a filter (slots 0 = region, 1 = count).
fn workload_e() -> PhysicalPlan {
    PhysicalPlan::Aggregate {
        input: Box::new(PhysicalPlan::Filter {
            input: Box::new(PhysicalPlan::EntityScan {
                entity_id: ix_account(),
                columns: vec![col(2, 0)],
            }),
            predicate: Expr::Binary {
                left: Box::new(Expr::Slot { slot: SlotId(0) }),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Literal {
                    value: Value::Int64(7),
                }),
            },
        }),
        group_by: vec![],
        aggregates: vec![AggregateSpec {
            function: AggregateFunction::Count,
            input: None,
            output: SlotId(1),
        }],
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

/// Executes `plan` once (at `snapshot`, or the latest snapshot).
fn run_at(db: &Database, plan: &PhysicalPlan, snapshot: Option<CommitTs>) -> Result<Run> {
    let mut cursor = match snapshot {
        Some(ts) => db.execute_at(plan.clone(), ts)?,
        None => db.execute(plan.clone())?,
    };
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

/// Executes `plan` once at the latest snapshot.
fn run(db: &Database, plan: &PhysicalPlan) -> Result<Run> {
    run_at(db, plan, None)
}

/// Rounds the samples of one measurement are split into for `round_medians` (their spread
/// shows run-to-run stability, which one median hides).
const ROUNDS: usize = 7;

/// Summary statistics of millisecond samples: percentiles, mean, min, max, standard deviation
/// and the medians of [`ROUNDS`] consecutive rounds.
fn summary(samples: &[f64]) -> serde_json::Map<String, serde_json::Value> {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let variance =
        samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / samples.len().max(2) as f64;
    let rounds: Vec<f64> = samples
        .chunks(samples.len().div_ceil(ROUNDS))
        .map(|round| {
            let mut round = round.to_vec();
            round.sort_by(f64::total_cmp);
            round[round.len() / 2]
        })
        .collect();
    let mut map = serde_json::Map::new();
    for (key, value) in [
        ("p50_ms", json!(percentile(0.5))),
        ("p95_ms", json!(percentile(0.95))),
        ("p99_ms", json!(percentile(0.99))),
        ("mean_ms", json!(mean)),
        ("min_ms", json!(sorted[0])),
        ("max_ms", json!(sorted[sorted.len() - 1])),
        ("stddev_ms", json!(variance.sqrt())),
        ("round_medians_ms", json!(rounds)),
    ] {
        map.insert(key.into(), value);
    }
    map
}

/// Warms up for at least `warmup` (and 10 runs), then times `iters` executions at `snapshot`:
/// the [`summary`] statistics, input rows per second at the median, rows, checksum and peak
/// memory.
fn measure_at(
    db: &Database,
    plan: &PhysicalPlan,
    snapshot: Option<CommitTs>,
    settings: (usize, Duration),
    input_rows: u64,
) -> Result<serde_json::Value> {
    let (iters, warmup) = settings;
    let start = Instant::now();
    let mut warm = 0;
    while warm < 10 || start.elapsed() < warmup {
        run_at(db, plan, snapshot)?;
        warm += 1;
    }
    let mut samples = Vec::with_capacity(iters);
    let mut last = run_at(db, plan, snapshot)?;
    for _ in 0..iters {
        let start = Instant::now();
        last = run_at(db, plan, snapshot)?;
        samples.push(start.elapsed().as_secs_f64() * 1e3);
    }
    let mut report = summary(&samples);
    let median = report["p50_ms"].as_f64().unwrap_or(f64::NAN);
    for (key, value) in [
        ("iterations", json!(iters)),
        ("warmup_runs", json!(warm)),
        ("input_rows", json!(input_rows)),
        (
            "input_rows_per_s",
            json!(input_rows as f64 / (median / 1e3)),
        ),
        ("rows_out", json!(last.rows)),
        ("checksum", json!(last.checksum)),
        ("peak_memory_bytes", json!(last.peak_memory_bytes)),
    ] {
        report.insert(key.into(), value);
    }
    Ok(serde_json::Value::Object(report))
}

/// [`measure_at`] at the latest snapshot.
fn measure(
    db: &Database,
    plan: &PhysicalPlan,
    iters: usize,
    warmup: Duration,
    input_rows: u64,
) -> Result<serde_json::Value> {
    measure_at(db, plan, None, (iters, warmup), input_rows)
}

/// Builds every index of [`dataset::INDEXES`] over the loaded rows: build time, entries
/// (counted by a full index scan, which must equal the table's rows) and the id of each.
fn build_indexes(
    db: &Database,
    scale: u64,
) -> Result<(serde_json::Value, Vec<(&'static str, u64)>)> {
    let mut report = serde_json::Map::new();
    let mut ids = Vec::new();
    for index in &dataset::INDEXES {
        let start = Instant::now();
        let definition = db.create_index(index.spec())?;
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let full = PhysicalPlan::IndexScan {
            index_id: definition.id.0,
            bounds: IndexBounds::all(),
            direction: ScanDirection::Forward,
            columns: vec![],
            limit: None,
        };
        let entries = run(db, &full)?.rows;
        let rows = dataset::rows(index.table, scale);
        report.insert(
            index.name.into(),
            json!({"sql": index.sql, "ms": ms, "entries": entries, "table_rows": rows,
                   "rows_per_s": rows as f64 / (ms / 1e3)}),
        );
        ids.push((index.name, definition.id.0));
    }
    Ok((serde_json::Value::Object(report), ids))
}

/// Rows inserted per write-overhead measurement.
const WRITE_ROWS: u64 = 20_000;
/// Rows per commit in the write-overhead measurement.
const WRITE_BATCH: u64 = 100;

/// Inserts [`WRITE_ROWS`] `ix_account`-shaped rows (100 per commit) into fresh databases with
/// 0, 1 and 3 indexes; reports rows per second and the overhead relative to no index.
fn write_overhead() -> Result<serde_json::Value> {
    let entity = ix_account();
    let index = |name: &str, fields: &[u32], include: &[u32]| IndexSpec {
        name: name.into(),
        entity_id: entity,
        columns: fields
            .iter()
            .map(|field| IndexColumn {
                field_id: *field,
                direction: Direction::Asc,
            })
            .collect(),
        include: include.to_vec(),
    };
    let sets: [(&str, Vec<IndexSpec>); 3] = [
        ("0_indexes", vec![]),
        ("1_index", vec![index("w_balance", &[3], &[])]),
        (
            "3_indexes",
            vec![
                index("w_balance", &[3], &[]),
                index("w_owner", &[4], &[3]),
                index("w_region_balance", &[2, 3], &[]),
            ],
        ),
    ];
    let mut report = serde_json::Map::new();
    let mut baseline = None;
    for (name, specs) in sets {
        let dir = tempdir()?;
        let db = Database::open(dir.path())?;
        for spec in specs {
            db.create_index(spec)?;
        }
        let table = dataset::by_name("ix_account");
        let start = Instant::now();
        let mut first = 1;
        while first <= WRITE_ROWS {
            let mut tx = db.begin();
            for id in first..first + WRITE_BATCH {
                tx.put(
                    RowId::from_primary_key(entity, id as i64),
                    dataset::row(table, id, 1),
                );
            }
            db.commit(tx)?;
            first += WRITE_BATCH;
        }
        let seconds = start.elapsed().as_secs_f64();
        let rate = WRITE_ROWS as f64 / seconds;
        let base = *baseline.get_or_insert(rate);
        report.insert(
            name.into(),
            json!({"rows": WRITE_ROWS, "rows_per_commit": WRITE_BATCH, "seconds": seconds,
                   "rows_per_s": rate, "slowdown_vs_no_index": base / rate}),
        );
        db.close()?;
    }
    Ok(serde_json::Value::Object(report))
}

/// Workload D at the latest snapshot and at the snapshot before 1% of the balances changed
/// (`AS OF`): the index serves both, older entries are kept until `VACUUM`.
fn as_of_read(
    db: &Database,
    balance_index: u64,
    scale: u64,
    settings: (usize, Duration),
) -> Result<serde_json::Value> {
    let before = db.latest_committed_ts();
    let table = dataset::by_name("ix_account");
    let rows = table.rows(scale);
    let mut tx = db.begin();
    for id in (1..=rows).step_by(100) {
        let moved: Row =
            dataset::row(table, id, scale).with_field(3, Value::Int64((D_FROM as u64 + id) as i64));
        tx.put(RowId::from_primary_key(table.entity, id as i64), moved);
    }
    db.commit(tx)?;
    let plan = workload_d(balance_index);
    Ok(json!({
        "changed_rows": rows.div_ceil(100),
        "latest": measure_at(db, &plan, None, settings, 100)?,
        "as_of_before_change": measure_at(db, &plan, Some(before), settings, 100)?,
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

    let (index_build, index_ids) = build_indexes(&db, args.scale)?;
    let index_id = |name: &str| {
        index_ids
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, id)| *id)
            .expect("index built")
    };
    let (owner_index, balance_index) = (index_id("idx_ix_owner"), index_id("idx_ix_balance"));
    let accounts = dataset::rows("ix_account", args.scale);
    let join_rows =
        dataset::rows("bench_customer", args.scale) + dataset::rows("bench_orders", args.scale);
    let settings = (args.iters, args.warmup);
    let workloads = json!({
        "A_hash_join_topk": measure(&db, &workload_a(), args.iters, args.warmup, join_rows)?,
        "B_hash_join_aggregate_topk": measure(&db, &workload_b(), args.iters, args.warmup, join_rows)?,
        "C_covering_index_eq": measure(&db, &workload_c(owner_index), args.iters, args.warmup, accounts / dataset::OWNERS)?,
        "D_index_range_order_limit": measure(&db, &workload_d(balance_index), args.iters, args.warmup, 100)?,
        "E_seq_scan_region_eq": measure(&db, &workload_e(), args.iters, args.warmup, accounts)?,
    });
    // Changes balances: runs after every other measurement of ix_account.
    let as_of = as_of_read(&db, balance_index, args.scale, settings)?;
    let report = json!({
        "milestone": "2.4.0",
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
            "ix_account": accounts,
            "iterations": args.iters,
            "warmup_ms": args.warmup.as_millis() as u64,
        },
        "load": {"rows": loaded, "seconds": load_seconds, "rows_per_s": loaded as f64 / load_seconds},
        "analyze": analyze,
        "index_build": index_build,
        "workloads": workloads,
        "as_of_index_read": as_of,
        "write_overhead": write_overhead()?,
        "schema_evolution": schema::report(args.scale, args.iters, args.warmup)?,
    });
    let text = serde_json::to_string_pretty(&report)?;
    println!("{text}");
    if let Some(path) = args.output {
        std::fs::write(path, text + "\n")?;
    }
    Ok(())
}
