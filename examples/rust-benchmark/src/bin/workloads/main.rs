//! Milestone 2.5.4.1 workload benchmark (workloads A–H of 2.4.0 plus I–O, the commit order, change
//! log and projection runtime, in [`consistency`]).
//!
//! Since 2.5.4.1 the timed sections run on the `adb-bench` harness ([`harness`]): CPU-bound cases use
//! the release plan, loads, commits and recovery the durable plan, and the report is schema 3 with
//! every earlier key kept. `--iters` and `--warmup-ms` are accepted for old command lines; the plan
//! decides how often a case runs. `--data-dir` (or `ADB_BENCH_DATA_DIR`) is where durable cases
//! write, and `ADB_BENCH_QUICK=1` selects the quick plans.
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

mod consistency;
mod dataset;
mod durable;
mod harness;
mod schema;

use std::{env, path::PathBuf};

use adb_bench::report::{plan_json, rate_per_second};
use adb_bench::{measure, measure_with_setup, Report, SampleStats};
use adb_core::{CommitTs, Row, RowId, Value};
use adb_engine::{AnalyzeOptions, Database};
use adb_execution::{
    AggregateFunction, AggregateSpec, BinaryOp, ColumnVector, Expr, JoinKey, JoinType,
    PhysicalPlan, ScanColumn, SlotId, SortKey,
};
use adb_index::{IndexBounds, ScanDirection, ValueBound};
use harness::{case, durable_plan, heavy_plan, invocations, tempdir};
use serde_json::json;

/// Result type of the benchmark (any error aborts it).
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
pub fn ix_account() -> u64 {
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

/// Times `plan` at `snapshot` with the CPU-bound measurement plan and returns its case: the
/// timing keys, input rows per second at the median, rows, checksum and peak memory.
fn measure_at(
    db: &Database,
    plan: &PhysicalPlan,
    snapshot: Option<CommitTs>,
    input_rows: u64,
) -> Result<serde_json::Value> {
    let measurement_plan = harness::cpu_plan();
    let measured = measure(&measurement_plan, || {
        run_at(db, plan, snapshot).expect("the workload executes")
    });
    let stats = SampleStats::from_measured(&measured);
    let last = measured.last;
    Ok(case(
        &stats,
        vec![
            ("iterations", json!(invocations(&stats, &measurement_plan))),
            ("warmup_runs", json!(measurement_plan.warmup_invocations)),
            ("input_rows", json!(input_rows)),
            (
                "input_rows_per_s",
                json!(rate_per_second(&stats, input_rows as f64)),
            ),
            ("rows_out", json!(last.rows)),
            ("checksum", json!(last.checksum)),
            ("peak_memory_bytes", json!(last.peak_memory_bytes)),
        ],
    ))
}

/// [`measure_at`] at the latest snapshot.
fn measure_latest(
    db: &Database,
    plan: &PhysicalPlan,
    input_rows: u64,
) -> Result<serde_json::Value> {
    measure_at(db, plan, None, input_rows)
}

/// `ANALYZE` of every table, one case each (`rows`, `ms`, `exact` and timing), plus a `total`.
/// `ANALYZE` can be repeated, so it is measured like any CPU-bound case.
fn analyze_tables(
    db: &Database,
    loaded: u64,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let plan = harness::cpu_plan();
    let mut report = serde_json::Map::new();
    let mut total_ms = 0.0;
    for table in &dataset::TABLES {
        let measured = measure(&plan, || {
            db.analyze(table.entity, &AnalyzeOptions::default())
                .expect("analyze")
        });
        let stats = SampleStats::from_measured(&measured);
        let ms = stats.median_ns / 1e6;
        total_ms += ms;
        report.insert(
            table.name.into(),
            case(
                &stats,
                vec![
                    ("rows", json!(measured.last.row_count)),
                    ("ms", json!(ms)),
                    ("exact", json!(measured.last.exact)),
                ],
            ),
        );
    }
    report.insert(
        "total".into(),
        json!({"rows": loaded, "ms": total_ms, "rows_per_s": loaded as f64 / (total_ms / 1e3)}),
    );
    Ok(report)
}

/// Builds every index of [`dataset::INDEXES`] over the loaded rows: build time (a case, measured
/// with the plan's setup/run split: dropping the index beforehand is not timed), entries (counted by
/// a full index scan, which must equal the table's rows) and the id of each. The last build stays.
fn build_indexes(
    db: &Database,
    scale: u64,
) -> Result<(serde_json::Value, Vec<(&'static str, u64)>)> {
    let plan = harness::cpu_plan();
    let mut report = serde_json::Map::new();
    let mut ids = Vec::new();
    for index in &dataset::INDEXES {
        let measured = measure_with_setup(
            &plan,
            || {
                let _ = db.drop_index(index.name);
            },
            |()| db.create_index(index.spec()).expect("index builds"),
        );
        let stats = SampleStats::from_measured(&measured);
        let definition = measured.last;
        let ms = stats.median_ns / 1e6;
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
            case(
                &stats,
                vec![
                    ("sql", json!(index.sql)),
                    ("ms", json!(ms)),
                    ("entries", json!(entries)),
                    ("table_rows", json!(rows)),
                    ("rows_per_s", json!(rate_per_second(&stats, rows as f64))),
                ],
            ),
        );
        ids.push((index.name, definition.id.0));
    }
    Ok((serde_json::Value::Object(report), ids))
}

/// Workload D at the latest snapshot and at the snapshot before 1% of the balances changed
/// (`AS OF`): the index serves both, older entries are kept until `VACUUM`.
fn as_of_read(db: &Database, balance_index: u64, scale: u64) -> Result<serde_json::Value> {
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
        "latest": measure_at(db, &plan, None, 100)?,
        "as_of_before_change": measure_at(db, &plan, Some(before), 100)?,
    }))
}

/// Command-line options.
struct Args {
    /// Scale factor of the dataset.
    scale: u64,
    /// Report path (measure mode).
    output: Option<PathBuf>,
    /// Directory to prepare (prepare mode).
    prepare: Option<PathBuf>,
    /// Directory the durable cases write to.
    data_dir: Option<PathBuf>,
}

/// Parses `--scale N`, `--output PATH`, `--prepare DIR` and `--data-dir DIR`. `--iters N` and
/// `--warmup-ms N` are validated and ignored: the measurement plan decides.
fn parse_args() -> Result<Args> {
    let mut args = Args {
        scale: 1,
        output: None,
        prepare: None,
        data_dir: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    for pair in raw.chunks(2) {
        match (pair[0].as_str(), pair.get(1)) {
            ("--iters", Some(value)) => {
                value.parse::<usize>()?;
            }
            ("--warmup-ms", Some(value)) => {
                value.parse::<u64>()?;
            }
            ("--scale", Some(value)) => args.scale = value.parse::<u64>()?.clamp(1, 1_000),
            ("--output", Some(value)) => args.output = Some(PathBuf::from(value)),
            ("--prepare", Some(value)) => args.prepare = Some(PathBuf::from(value)),
            ("--data-dir", Some(value)) => args.data_dir = Some(PathBuf::from(value)),
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
    let context = harness::init(args.data_dir.clone());
    if let Some(dir) = &args.prepare {
        return prepare(dir, args.scale);
    }
    let cpu = harness::cpu_plan();
    let mut document = Report::new("workloads", "2.5.4.1", &cpu, &context.environment);
    document.set("plan_durable", plan_json(&durable_plan()));
    document.set("plan_heavy", plan_json(&heavy_plan()));
    document.set("storage", context.storage().to_json());
    let probe_dir = tempdir()?;
    let probe_before = context.probe(probe_dir.path());

    // Loading is durable work: measured with the durable plan, a fresh database each time
    // (creating it is not timed). The last loaded database serves every later section.
    eprintln!("workloads: loading the dataset");
    let load = measure_with_setup(
        &heavy_plan(),
        || {
            let dir = tempdir().expect("data directory");
            let db = Database::open(dir.path()).expect("open");
            (dir, db)
        },
        |(dir, db)| {
            let (rows, _) = dataset::load(&db, args.scale).expect("load");
            (dir, db, rows)
        },
    );
    let load_stats = SampleStats::from_measured(&load);
    let (_dir, db, loaded) = load.last;
    let load_seconds = load_stats.median_ns / 1e9;
    let load_case = case(
        &load_stats,
        vec![
            ("rows", json!(loaded)),
            ("seconds", json!(load_seconds)),
            (
                "rows_per_s",
                json!(rate_per_second(&load_stats, loaded as f64)),
            ),
        ],
    );

    eprintln!("workloads: ANALYZE");
    let mut analyze = analyze_tables(&db, loaded)?;
    // HyperLogLog check: sk_events ids are unique, so the distinct count should equal the rows.
    let events = dataset::by_name("sk_events");
    let ids = db
        .statistics(events.entity)
        .and_then(|s| s.column(1).map(|c| c.distinct_count));
    analyze.insert(
        "sk_events_distinct_ids".into(),
        json!({"estimated": ids, "actual": events.rows(args.scale)}),
    );

    eprintln!("workloads: index builds");
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
    eprintln!("workloads: A-E");
    let workloads = json!({
        "A_hash_join_topk": measure_latest(&db, &workload_a(), join_rows)?,
        "B_hash_join_aggregate_topk": measure_latest(&db, &workload_b(), join_rows)?,
        "C_covering_index_eq": measure_latest(&db, &workload_c(owner_index), accounts / dataset::OWNERS)?,
        "D_index_range_order_limit": measure_latest(&db, &workload_d(balance_index), 100)?,
        "E_seq_scan_region_eq": measure_latest(&db, &workload_e(), accounts)?,
    });
    // Changes balances: runs after every other measurement of ix_account.
    eprintln!("workloads: AS OF index read");
    let as_of = as_of_read(&db, balance_index, args.scale)?;
    let write_overhead = durable::write_overhead()?;
    eprintln!("workloads: schema evolution F-H");
    let schema_evolution = schema::report(args.scale)?;
    eprintln!("workloads: consistency I-O");
    let consistency = consistency::report(args.scale)?;

    let probe_after = context.probe(probe_dir.path());
    if let (Some(before), Some(after)) = (&probe_before, &probe_after) {
        document.set(
            "durable_write",
            durable::summary(before, after, &write_overhead, &consistency),
        );
    }

    document.set("path", json!("rust_native"));
    document.set(
        "host",
        json!({
            "os": env::consts::OS,
            "arch": env::consts::ARCH,
            "cpu": cpu_model(),
            "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        }),
    );
    document.set(
        "setup",
        json!({
            "scale": args.scale,
            "bench_customer": dataset::rows("bench_customer", args.scale),
            "bench_orders": dataset::rows("bench_orders", args.scale),
            "ix_account": accounts,
            "iterations": cpu.total_samples(),
            "warmup_ms": cpu.min_sample.as_millis() as u64,
        }),
    );
    document.set("load", load_case);
    document.set("analyze", json!(analyze));
    document.set("index_build", index_build);
    document.set("workloads", workloads);
    document.set("as_of_index_read", as_of);
    document.set("write_overhead", write_overhead);
    document.set("schema_evolution", schema_evolution);
    document.set("consistency", consistency);
    if let (Some(before), Some(after)) = (&probe_before, &probe_after) {
        document.set(
            "fsync_probe",
            json!({"before": before.to_json(), "after": after.to_json(), "drift_percent": before.drift_percent(after)}),
        );
    }
    let document = document.finish();
    let problems = adb_bench::report::validate(&document);
    if !problems.is_empty() {
        return Err(format!("the report breaks its own schema: {problems:?}").into());
    }
    let text = serde_json::to_string_pretty(&document)?;
    println!("{text}");
    if let Some(path) = args.output {
        std::fs::write(path, text + "\n")?;
    }
    Ok(())
}
