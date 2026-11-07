//! The `memory` family (Milestone 2.5.4.1): how many bytes and allocation calls the engine's hot
//! paths cost, and the heap and resident-set high-water marks of a load and a query.
//!
//! Allocation, unlike time, does not depend on the load of the machine, so these figures are
//! compared exactly (see `scripts/check-benchmarks.py`) and a change in them is a real change in
//! the code. The binary installs a counting allocator, which would distort timings, so it measures
//! nothing else. Counters are per thread: work the engine does on its own threads (group commit)
//! is not included, and the report says so.
//!
//! ```text
//! cargo run --release -p adb-benchmark-rust --bin memory -- --rows 2000 --output out.json
//! ```

use std::{env, hint::black_box, path::PathBuf, time::Duration};

use adb_bench::alloc::{count, peak_bytes, peak_rss_bytes, reset_peak};
use adb_bench::report::plan_json;
use adb_bench::{Context, CountingAllocator, MeasurementPlan, Report};
use adb_core::{Row, RowId, Value};
use adb_engine::{ChangeCursor, ChangeFilter, Database};
use adb_execution::{
    AggregateFunction, AggregateSpec, JoinKey, JoinType, PhysicalPlan, ScanColumn, SlotId,
};
use serde_json::{json, Map, Value as Json};

/// Counts every allocation of this process (per thread; see [`adb_bench::alloc`]).
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Result type of the benchmark (any error aborts it).
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Command-line options.
struct Args {
    /// Rows per entity.
    rows: u64,
    /// Repetitions of each case, to show that the figures do not move.
    repeats: u32,
    /// Report path.
    output: Option<PathBuf>,
    /// Directory the databases are created in.
    data_dir: Option<PathBuf>,
}

/// Parses `--rows`, `--repeats`, `--output` and `--data-dir`.
fn parse_args() -> Result<Args> {
    let mut args = Args {
        rows: 2_000,
        repeats: 3,
        output: None,
        data_dir: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    for pair in raw.chunks(2) {
        match (pair[0].as_str(), pair.get(1)) {
            ("--rows", Some(v)) => args.rows = v.parse()?,
            ("--repeats", Some(v)) => args.repeats = v.parse::<u32>()?.max(1),
            ("--output", Some(v)) => args.output = Some(PathBuf::from(v)),
            ("--data-dir", Some(v)) => args.data_dir = Some(PathBuf::from(v)),
            (other, _) => return Err(format!("unknown or incomplete argument {other}").into()),
        }
    }
    Ok(args)
}

/// Benchmark row: field 1 = value, field 2 = its string form.
fn row(value: u64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(value as i64))
        .with_field(2, Value::String(format!("row-{value}")))
}

/// Field 1 of every row, into slot 0.
fn field_1() -> Vec<ScanColumn> {
    vec![ScanColumn {
        field_id: 1,
        slot: SlotId(0),
    }]
}

/// Executes `plan` to the end; returns the rows and the peak bytes its blocking operators held.
fn run(db: &Database, plan: PhysicalPlan) -> (usize, u64) {
    let mut cursor = db.execute(plan).expect("plan executes");
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch().expect("batch") {
        rows += batch.len();
        black_box(&batch);
    }
    (rows, cursor.profile().peak_memory_bytes)
}

/// Measures one case: `ops` invocations of `f`, `repeats` times, after one warm-up round.
/// Returns the case's JSON: bytes and calls per operation (the median repeat), and how far the
/// repeats were apart.
fn alloc_case(ops: u64, repeats: u32, mut f: impl FnMut()) -> Json {
    for _ in 0..ops {
        f(); // warm-up: lazily built state is allocated once and must not count
    }
    let mut bytes = Vec::new();
    let mut calls = Vec::new();
    for _ in 0..repeats {
        let ((), usage) = count(|| {
            for _ in 0..ops {
                f();
            }
        });
        let (b, c) = usage.per_run(ops);
        bytes.push(b);
        calls.push(c);
    }
    let median = |values: &[f64]| adb_bench::stats::median(values);
    let (b, c) = (median(&bytes), median(&calls));
    let spread = |values: &[f64], m: f64| {
        let (lo, hi) = values
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        if m > 0.0 {
            (hi - lo) / m * 100.0
        } else {
            0.0
        }
    };
    let bytes_spread = spread(&bytes, b);
    println!("  bytes/op={b:.0} calls/op={c:.1} spread%={bytes_spread:.2}");
    json!({
        "value": b,
        "bytes_per_op": b,
        "calls_per_op": c,
        "ops": ops,
        "repeats": repeats,
        "repeat_spread_percent": bytes_spread,
        "deterministic": bytes_spread == 0.0 && spread(&calls, c) == 0.0,
    })
}

/// Runs every case and writes the report.
fn main() -> Result<()> {
    let args = parse_args()?;
    let context = Context::new(args.data_dir.clone());
    // The "plan" of this family: `repeats` repetitions of a fixed number of operations.
    let plan = MeasurementPlan {
        warmup_invocations: 1,
        min_sample: Duration::ZERO,
        max_iterations: 1,
        batches: args.repeats,
        samples_per_batch: 1,
    };
    let mut report = Report::new("memory", "2.5.4.1", &plan, &context.environment);
    report.set("storage", context.storage().to_json());
    report.set("plan_detail", plan_json(&plan));
    report.set(
        "scope",
        json!(
            "allocations of the calling thread only; engine threads (group commit) are not counted"
        ),
    );
    println!("Adaptive DB 2.5.4.1 memory benchmark, rows={}", args.rows);

    let dir = context.tempdir()?;
    let db = Database::open(dir.path())?;

    // ---- heap and resident-set high-water mark of a bulk load ----------------------------------
    reset_peak();
    let base = adb_bench::alloc::live_bytes();
    for base_pk in (0..args.rows).step_by(100) {
        let mut tx = db.begin();
        for pk in base_pk..(base_pk + 100).min(args.rows) {
            tx.put(RowId::compose(2, pk), row(pk));
        }
        db.commit(tx)?;
    }
    let load_peak = peak_bytes().saturating_sub(base);
    println!("load heap_peak_bytes={load_peak}");

    let mut cases = Map::new();
    let mut insert = |name: &str, node: Json| {
        cases.insert(name.to_string(), node);
    };

    println!("point_lookup");
    let mut key = 0u64;
    insert(
        "point_lookup",
        alloc_case(1_000, args.repeats, || {
            key = (key + 7919) % args.rows;
            black_box(db.get(RowId::compose(2, key)).expect("lookup"));
        }),
    );

    println!("entity_scan");
    insert(
        "entity_scan",
        alloc_case(10, args.repeats, || {
            black_box(run(
                &db,
                PhysicalPlan::EntityScan {
                    entity_id: 2,
                    columns: field_1(),
                },
            ));
        }),
    );

    println!("change_feed_read");
    insert(
        "change_feed_read",
        alloc_case(10, args.repeats, || {
            let mut cursor = ChangeCursor::BEGINNING;
            loop {
                let batch = db
                    .read_changes(cursor, 1_000, &ChangeFilter::all())
                    .expect("changes");
                if batch.events.is_empty() {
                    break;
                }
                cursor = batch.next;
            }
        }),
    );

    println!("plan_json_parse");
    let plan_text = serde_json::to_string(&PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::EntityScan {
            entity_id: 2,
            columns: field_1(),
        }),
        limit: 100,
    })?;
    insert(
        "plan_json_parse",
        alloc_case(1_000, args.repeats, || {
            black_box(serde_json::from_str::<PhysicalPlan>(&plan_text).expect("parses"));
        }),
    );

    // ---- a small relational pipeline and the query's own peak memory ---------------------------
    let mut tx = db.begin();
    for id in 0..50u64 {
        tx.put(
            RowId::compose(10, id),
            Row::new()
                .with_field(1, Value::Int64(id as i64))
                .with_field(2, Value::String(format!("group-{id}"))),
        );
    }
    db.commit(tx)?;
    let mut tx = db.begin();
    for id in 0..args.rows {
        tx.put(
            RowId::compose(11, id),
            Row::new()
                .with_field(1, Value::Int64(id as i64))
                .with_field(2, Value::Int64((id % 50) as i64))
                .with_field(3, Value::Int64(((id * 7919) % 1000) as i64)),
        );
    }
    db.commit(tx)?;
    let col = |field_id, slot| ScanColumn {
        field_id,
        slot: SlotId(slot),
    };
    let pipeline = || PhysicalPlan::Aggregate {
        input: Box::new(PhysicalPlan::HashJoin {
            left: Box::new(PhysicalPlan::EntityScan {
                entity_id: 11,
                columns: vec![col(1, 0), col(2, 1), col(3, 2)],
            }),
            right: Box::new(PhysicalPlan::EntityScan {
                entity_id: 10,
                columns: vec![col(1, 3), col(2, 4)],
            }),
            join_type: JoinType::Inner,
            keys: vec![JoinKey {
                left: SlotId(1),
                right: SlotId(3),
            }],
            residual: None,
        }),
        group_by: vec![SlotId(4)],
        aggregates: vec![AggregateSpec {
            function: AggregateFunction::Sum,
            input: Some(SlotId(2)),
            output: SlotId(5),
        }],
    };
    println!("join_aggregate");
    let (_, query_peak) = run(&db, pipeline());
    let mut node = alloc_case(5, args.repeats, || {
        black_box(run(&db, pipeline()));
    });
    node["query_peak_memory_bytes"] = json!(query_peak);
    insert("join_aggregate", node);

    // ---- commits: the calling thread's allocation per commit (fsync is not what is measured) ----
    println!("commit_single_row");
    let mut next = 0u64;
    insert(
        "commit_single_row",
        alloc_case(100, args.repeats, || {
            let mut tx = db.begin();
            tx.put(RowId::compose(3, next), row(next));
            next += 1;
            db.commit(tx).expect("commit");
        }),
    );
    println!("commit_100_rows");
    insert(
        "commit_100_rows",
        alloc_case(10, args.repeats, || {
            let mut tx = db.begin();
            for _ in 0..100 {
                tx.put(RowId::compose(4, next), row(next));
                next += 1;
            }
            db.commit(tx).expect("commit");
        }),
    );

    for (name, node) in cases {
        report.add_plain_case(&name, node.as_object().cloned().unwrap_or_default(), None);
    }
    report.set("load_heap_peak_bytes", json!(load_peak));
    report.set("peak_rss_bytes", json!(peak_rss_bytes()));
    report.set("rows", json!(args.rows));
    let document = report.finish();
    let problems = adb_bench::report::validate(&document);
    if !problems.is_empty() {
        return Err(format!("the report breaks its own schema: {problems:?}").into());
    }
    let text = serde_json::to_string_pretty(&document)?;
    if let Some(path) = args.output {
        std::fs::write(&path, text + "\n")?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
