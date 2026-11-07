//! Adaptive DB data-plane benchmark (Milestone 2.5.4.1; the sections are those of 2.1.3).
//!
//! Since 2.5.4.1 every section runs on the `adb-bench` harness: a measurement plan (warm-up,
//! calibration, batches), the median of the batch medians as the headline, the batch MAD as the
//! stability input, and a record of the machine and of the storage under the data directory. Reports
//! are schema 3 and keep every key the earlier ones had. Durable sections (load, commits,
//! recovery) use the durable plan and are bracketed by an fsync probe; the others use the CPU-bound
//! plan. `ADB_BENCH_QUICK=1` selects the quick plans (a smoke run), `--data-dir` (or
//! `ADB_BENCH_DATA_DIR`) the directory the durable sections write to.
//!
//! Storage sections (comparable since 2.0.2): bulk load, point lookup, entity scan, change
//! feed, single-row commits with and without concurrency (group commit), heap size under update
//! churn and recovery time. Relational sections (2.1): hash join, nested-loop join, GROUP BY
//! with SUM, full sort, TopK, and plan wire v2 encoding / decoding + validation.

use std::{env, hint::black_box, path::PathBuf, sync::Arc, thread};

use adb_bench::report::{plan_json, rate_per_second};
use adb_bench::{measure, measure_with_setup, Context, Report, SampleStats};
use adb_core::{Row, RowId, Value};
use adb_engine::{ChangeCursor, ChangeFilter, Database};
use adb_execution::{
    AggregateFunction, AggregateSpec, BinaryOp, Expr, JoinKey, JoinType, PhysicalPlan, ScanColumn,
    SlotId, SortKey,
};
use serde_json::{json, Map, Value as Json};
use tempfile::TempDir;

/// Result type of the benchmark (any error aborts it).
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Command-line options.
struct Args {
    /// Rows per entity.
    rows: u64,
    /// Iterations of the point-lookup and plan-parse sections.
    iters: usize,
    /// Writer threads of the concurrent-commit section.
    threads: u64,
    /// Rows of the dimension table (= groups) in the relational sections.
    groups: u64,
    /// Outer rows of the nested-loop join (it compares `nested_rows x groups` pairs).
    nested_rows: u64,
    /// Repetitions of each relational section (the mean is reported).
    relational_iters: u32,
    /// Optional JSON report path.
    output: Option<PathBuf>,
    /// Directory the durable sections write to (default: `ADB_BENCH_DATA_DIR`, then the system
    /// temporary directory).
    data_dir: Option<PathBuf>,
}

/// Parses `--rows`, `--iters`, `--threads`, `--groups`, `--nested-rows`, `--relational-iters`,
/// `--output` and `--data-dir`. `--iters` and `--relational-iters` are kept so old command lines
/// still run; the measurement plan now decides how often a case is repeated.
fn parse_args() -> Result<Args> {
    let mut args = Args {
        rows: 5_000,
        iters: 1_000,
        threads: 8,
        groups: 100,
        nested_rows: 1_000,
        relational_iters: 5,
        output: None,
        data_dir: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        let value = raw.get(i + 1).ok_or("missing argument value")?;
        match raw[i].as_str() {
            "--rows" => args.rows = value.parse()?,
            "--iters" => args.iters = value.parse()?,
            "--threads" => args.threads = value.parse()?,
            "--groups" => args.groups = value.parse()?,
            "--nested-rows" => args.nested_rows = value.parse()?,
            "--relational-iters" => args.relational_iters = value.parse()?,
            "--output" => args.output = Some(PathBuf::from(value)),
            "--data-dir" => args.data_dir = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument: {other}").into()),
        }
        i += 2;
    }
    Ok(args)
}

/// Benchmark row: field 1 = value, field 2 = its string form.
fn row(value: u64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(value as i64))
        .with_field(2, Value::String(format!("row-{value}")))
}

/// Field 1 of every row, read into slot 0.
fn field_1() -> Vec<ScanColumn> {
    vec![ScanColumn {
        field_id: 1,
        slot: SlotId(0),
    }]
}

/// Executes `plan` and counts the rows it returns.
fn count_rows(db: &Database, plan: PhysicalPlan) -> Result<usize> {
    let mut cursor = db.execute(plan)?;
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch()? {
        rows += batch.len();
        black_box(&batch);
    }
    Ok(rows)
}

/// Shorthand for a scan column mapping.
fn col(field_id: u32, slot: u32) -> ScanColumn {
    ScanColumn {
        field_id,
        slot: SlotId(slot),
    }
}

/// Fact table (entity 11): slots 0 = id, 1 = group, 2 = amount.
fn facts() -> PhysicalPlan {
    PhysicalPlan::EntityScan {
        entity_id: 11,
        columns: vec![col(1, 0), col(2, 1), col(3, 2)],
    }
}

/// Dimension table (entity 10): slots 3 = id, 4 = label.
fn dims() -> PhysicalPlan {
    PhysicalPlan::EntityScan {
        entity_id: 10,
        columns: vec![col(1, 3), col(2, 4)],
    }
}

/// `amount DESC` sort key.
fn by_amount_desc() -> Vec<SortKey> {
    vec![SortKey {
        slot: SlotId(2),
        descending: true,
    }]
}

/// `facts JOIN dims ON facts.group = dims.id`, then `SUM(amount)` per label, best 10 first.
fn relational_pipeline() -> PhysicalPlan {
    PhysicalPlan::TopK {
        input: Box::new(PhysicalPlan::Aggregate {
            input: Box::new(PhysicalPlan::HashJoin {
                left: Box::new(facts()),
                right: Box::new(dims()),
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
        }),
        keys: vec![SortKey {
            slot: SlotId(5),
            descending: true,
        }],
        limit: 10,
    }
}

/// A database together with the temporary directory that holds it, so both are dropped together
/// (the directory last).
struct Fixture {
    /// The open database.
    db: Database,
    /// Its directory; removed when the fixture is dropped.
    _dir: TempDir,
}

/// Opens a database in a fresh directory below the context's data root.
fn fresh(context: &Context) -> Result<Fixture> {
    let dir = context.tempdir()?;
    let db = Database::open(dir.path())?;
    Ok(Fixture { db, _dir: dir })
}

/// Commits `rows` rows of `entity` in transactions of 100 rows.
fn load_entity(db: &Database, entity: u64, rows: u64) -> Result<()> {
    for base in (0..rows).step_by(100) {
        let mut tx = db.begin();
        for pk in base..(base + 100).min(rows) {
            tx.put(RowId::compose(entity, pk), row(pk));
        }
        db.commit(tx)?;
    }
    Ok(())
}

/// A report entry that is a plain object of `pairs`.
fn object(pairs: Vec<(&str, Json)>) -> Map<String, Json> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Relational sections over a fact table of `args.rows` rows and a dimension of `args.groups`.
/// Every entry keeps `rows_out` and `ms` (now the median) and gains the legacy timing keys and
/// a `timing` object.
fn relational(args: &Args, context: &Context, report: &Report) -> Result<Json> {
    let fixture = fresh(context)?;
    let db = &fixture.db;
    let groups = args.groups.max(1);
    let mut tx = db.begin();
    for id in 0..groups {
        tx.put(
            RowId::compose(10, id),
            Row::new()
                .with_field(1, Value::Int64(id as i64))
                .with_field(2, Value::String(format!("group-{id}"))),
        );
    }
    db.commit(tx)?;
    for base in (0..args.rows).step_by(500) {
        let mut tx = db.begin();
        for id in base..(base + 500).min(args.rows) {
            tx.put(
                RowId::compose(11, id),
                Row::new()
                    .with_field(1, Value::Int64(id as i64))
                    .with_field(2, Value::Int64((id % groups) as i64))
                    .with_field(3, Value::Int64(((id * 7919) % 1000) as i64)),
            );
        }
        db.commit(tx)?;
    }

    let plan = context.plan(false);
    let hash_join = PhysicalPlan::HashJoin {
        left: Box::new(facts()),
        right: Box::new(dims()),
        join_type: JoinType::Inner,
        keys: vec![JoinKey {
            left: SlotId(1),
            right: SlotId(3),
        }],
        residual: None,
    };
    let nested_loop_join = PhysicalPlan::NestedLoopJoin {
        left: Box::new(PhysicalPlan::Limit {
            input: Box::new(facts()),
            limit: args.nested_rows as usize,
        }),
        right: Box::new(dims()),
        join_type: JoinType::Inner,
        predicate: Some(Expr::Binary {
            left: Box::new(Expr::Slot { slot: SlotId(1) }),
            op: BinaryOp::Eq,
            right: Box::new(Expr::Slot { slot: SlotId(3) }),
        }),
    };
    let aggregate = PhysicalPlan::Aggregate {
        input: Box::new(facts()),
        group_by: vec![SlotId(1)],
        aggregates: vec![AggregateSpec {
            function: AggregateFunction::Sum,
            input: Some(SlotId(2)),
            output: SlotId(5),
        }],
    };
    let sort = PhysicalPlan::Sort {
        input: Box::new(facts()),
        keys: by_amount_desc(),
    };
    let top_k = PhysicalPlan::TopK {
        input: Box::new(facts()),
        keys: by_amount_desc(),
        limit: 10,
    };

    let mut out = Map::new();
    for (name, physical) in [
        ("hash_join", &hash_join),
        ("nested_loop_join", &nested_loop_join),
        ("aggregate_sum_group_by", &aggregate),
        ("sort", &sort),
        ("top_k", &top_k),
        ("join_aggregate_top_k", &relational_pipeline()),
    ] {
        let measured = measure(&plan, || {
            count_rows(db, physical.clone()).expect("relational plan executes")
        });
        let stats = SampleStats::from_measured(&measured);
        let ms = stats.median_ns / 1e6;
        println!(
            "{name} rows_out={} ms={ms:.2} mad%={:.1}",
            measured.last, stats.batch_mad_percent
        );
        out.insert(
            name.to_string(),
            report.case(
                &stats,
                object(vec![("rows_out", json!(measured.last)), ("ms", json!(ms))]),
            ),
        );
    }

    let pipeline = relational_pipeline();
    let encode = measure(&plan, || {
        adb_plan_wire::encode_json(&pipeline, adb_core::SchemaEpoch(0)).expect("encodes")
    });
    let encoded = encode.last.clone();
    let encode_stats = SampleStats::from_measured(&encode);
    let decode = measure(&plan, || {
        adb_plan_wire::decode_json(&encoded).expect("decodes and validates")
    });
    let decode_stats = SampleStats::from_measured(&decode);
    let encode_ops = rate_per_second(&encode_stats, 1.0);
    let decode_ops = rate_per_second(&decode_stats, 1.0);
    println!(
        "plan_wire_encode ops_s={encode_ops:.0} plan_wire_decode_validate ops_s={decode_ops:.0}"
    );
    out.insert("plan_wire_encode_ops_s".into(), json!(encode_ops));
    out.insert("plan_wire_decode_validate_ops_s".into(), json!(decode_ops));
    out.insert(
        "plan_wire_encode".into(),
        report.case(&encode_stats, object(vec![("ops_s", json!(encode_ops))])),
    );
    out.insert(
        "plan_wire_decode_validate".into(),
        report.case(&decode_stats, object(vec![("ops_s", json!(decode_ops))])),
    );
    out.insert(
        "setup".into(),
        json!({"fact_rows": args.rows, "groups": groups, "nested_rows": args.nested_rows, "iters": args.relational_iters}),
    );
    Ok(Json::Object(out))
}

/// Runs every benchmark section and optionally writes a JSON report.
fn main() -> Result<()> {
    let args = parse_args()?;
    let context = Context::new(args.data_dir.clone());
    let cpu_plan = context.plan(false);
    let durable_plan = context.plan(true);
    println!("Adaptive DB 2.5.4.1 Rust benchmark");
    println!(
        "rows={} threads={} governor={} data_root={} quick={}",
        args.rows,
        args.threads,
        context.environment.governor.as_deref().unwrap_or("unknown"),
        context.data_root.display(),
        context.quick
    );
    let mut report = Report::new("rust-local", "2.5.4.1", &cpu_plan, &context.environment);
    report.set("plan_durable", plan_json(&durable_plan));
    let storage = context.storage();
    report.set("storage", storage.to_json());
    let probe_dir = context.tempdir()?;
    let probe_before = context.probe(probe_dir.path());

    // ---- bulk load: 100 rows per transaction, 3 entities (durable) ---------------------------
    let loaded = 3 * args.rows;
    let load = measure(&durable_plan, || -> Fixture {
        let fixture = fresh(&context).expect("data directory");
        for entity in 1..=3u64 {
            load_entity(&fixture.db, entity, args.rows).expect("load");
        }
        fixture
    });
    let load_stats = SampleStats::from_measured(&load);
    let load_rate = rate_per_second(&load_stats, loaded as f64);
    println!(
        "load rows={loaded} ms={:.0} rows_s={load_rate:.0} mad%={:.1}",
        load_stats.median_ns / 1e6,
        load_stats.batch_mad_percent
    );
    report.set(
        "load",
        report.case(
            &load_stats,
            object(vec![
                ("rows", json!(loaded)),
                ("ms", json!(load_stats.median_ns / 1e6)),
                ("rows_s", json!(load_rate)),
            ]),
        ),
    );
    let db = &load.last.db;

    // ---- point lookup ------------------------------------------------------------------------
    let mut key = 0u64;
    let lookup = measure(&cpu_plan, || {
        key = (key + 7919) % args.rows;
        db.get(RowId::compose(2, key)).expect("lookup")
    });
    let lookup_stats = SampleStats::from_measured(&lookup);
    let lookup_ops = rate_per_second(&lookup_stats, 1.0);
    println!(
        "point_lookup ops_s={lookup_ops:.0} p50_ns={:.0} p95_ns={:.0} p99_ns={:.0}",
        lookup_stats.median_ns, lookup_stats.p95_ns, lookup_stats.p99_ns
    );
    report.set(
        "point_lookup",
        report.case(
            &lookup_stats,
            object(vec![
                ("ops_s", json!(lookup_ops)),
                ("p50_ns", json!(lookup_stats.median_ns)),
                ("p95_ns", json!(lookup_stats.p95_ns)),
                ("p99_ns", json!(lookup_stats.p99_ns)),
            ]),
        ),
    );

    // ---- entity scan vs full scan + filter (the 2.0.2 plan shape) ----------------------------
    let entity = measure(&cpu_plan, || {
        count_rows(
            db,
            PhysicalPlan::EntityScan {
                entity_id: 2,
                columns: field_1(),
            },
        )
        .expect("entity scan")
    });
    let entity_stats = SampleStats::from_measured(&entity);
    let full_plan = PhysicalPlan::Filter {
        input: Box::new(PhysicalPlan::Scan { columns: field_1() }),
        predicate: Expr::Binary {
            left: Box::new(Expr::Slot { slot: SlotId(0) }),
            op: BinaryOp::Ge,
            right: Box::new(Expr::Literal {
                value: Value::Int64(0),
            }),
        },
    };
    let full = measure(&cpu_plan, || {
        count_rows(db, full_plan.clone()).expect("full scan")
    });
    let full_stats = SampleStats::from_measured(&full);
    println!(
        "entity_scan rows={} ms={:.2} | full_scan+filter rows={} ms={:.2}",
        entity.last,
        entity_stats.median_ns / 1e6,
        full.last,
        full_stats.median_ns / 1e6
    );
    report.set(
        "entity_scan",
        report.case(
            &entity_stats,
            object(vec![
                ("rows", json!(entity.last)),
                ("ms", json!(entity_stats.median_ns / 1e6)),
            ]),
        ),
    );
    report.set(
        "full_scan_filter",
        report.case(
            &full_stats,
            object(vec![
                ("rows", json!(full.last)),
                ("ms", json!(full_stats.median_ns / 1e6)),
            ]),
        ),
    );

    // ---- plan JSON parsing (bare serde, comparable with 2.0.x) -------------------------------
    let plan_json_text = serde_json::to_string(&PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::EntityScan {
            entity_id: 2,
            columns: field_1(),
        }),
        limit: 100,
    })?;
    let parse = measure(&cpu_plan, || {
        serde_json::from_str::<PhysicalPlan>(&plan_json_text).expect("parses")
    });
    let parse_stats = SampleStats::from_measured(&parse);
    let parse_ops = rate_per_second(&parse_stats, 1.0);
    println!("physical_plan_json_parse ops_s={parse_ops:.0}");
    report.set("plan_json_parse_ops_s", json!(parse_ops));
    report.set(
        "plan_json_parse",
        report.case(&parse_stats, object(vec![("ops_s", json!(parse_ops))])),
    );

    // ---- change feed -------------------------------------------------------------------------
    let feed = measure(&cpu_plan, || {
        let mut cursor = ChangeCursor::BEGINNING;
        let mut events = 0u64;
        loop {
            let batch = db
                .read_changes(cursor, 1_000, &ChangeFilter::all())
                .expect("read changes");
            if batch.events.is_empty() {
                break;
            }
            events += batch.events.len() as u64;
            cursor = batch.next;
        }
        events
    });
    let feed_stats = SampleStats::from_measured(&feed);
    println!(
        "cdc_read events={} ms={:.1} events_s={:.0}",
        feed.last,
        feed_stats.median_ns / 1e6,
        rate_per_second(&feed_stats, feed.last as f64)
    );
    report.set(
        "cdc_read",
        report.case(
            &feed_stats,
            object(vec![
                ("events", json!(feed.last)),
                ("ms", json!(feed_stats.median_ns / 1e6)),
                (
                    "events_s",
                    json!(rate_per_second(&feed_stats, feed.last as f64)),
                ),
            ]),
        ),
    );

    // ---- single-row commits: 1 thread vs N threads (group commit; durable) -------------------
    let single_n = args.rows.min(2_000);
    let one = measure(&durable_plan, || -> Fixture {
        let fixture = fresh(&context).expect("data directory");
        for pk in 0..single_n {
            let mut tx = fixture.db.begin();
            tx.put(RowId::compose(1, pk), row(pk));
            fixture.db.commit(tx).expect("commit");
        }
        fixture
    });
    let one_stats = SampleStats::from_measured(&one);
    let commit_1 = rate_per_second(&one_stats, single_n as f64);

    let per_thread = single_n / args.threads.max(1);
    let many_total = per_thread * args.threads;
    let many = measure(&durable_plan, || -> Arc<Fixture> {
        let fixture = Arc::new(fresh(&context).expect("data directory"));
        let handles: Vec<_> = (0..args.threads)
            .map(|t| {
                let fixture = Arc::clone(&fixture);
                thread::spawn(move || -> std::result::Result<(), String> {
                    for pk in 0..per_thread {
                        let mut tx = fixture.db.begin();
                        tx.put(RowId::compose(t, pk), row(pk));
                        fixture.db.commit(tx).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("worker").expect("commit");
        }
        fixture
    });
    let many_stats = SampleStats::from_measured(&many);
    let commit_n = rate_per_second(&many_stats, many_total as f64);
    println!(
        "commit_single_row 1_thread_ops_s={commit_1:.0} {}_threads_ops_s={commit_n:.0}",
        args.threads
    );
    report.set(
        "commit_single_row",
        json!({
            "one_thread_ops_s": commit_1,
            "n_threads_ops_s": commit_n,
            "one_thread": report.case(&one_stats, object(vec![("ops_s", json!(commit_1))])),
            "n_threads": report.case(&many_stats, object(vec![("ops_s", json!(commit_n)), ("threads", json!(args.threads))])),
        }),
    );

    // ---- update churn: current heap must stay bounded ----------------------------------------
    let churn = fresh(&context)?;
    let mut tx = churn.db.begin();
    for pk in 0..100 {
        tx.put(RowId::compose(1, pk), row(0));
    }
    churn.db.commit(tx)?;
    let pages_before = churn.db.storage_stats()?.current_heap_pages;
    for round in 0..20 {
        let mut tx = churn.db.begin();
        for pk in 0..100 {
            tx.put(RowId::compose(1, pk), row(round));
        }
        churn.db.commit(tx)?;
    }
    let pages_after = churn.db.storage_stats()?.current_heap_pages;
    println!("churn_heap_pages before={pages_before} after_2000_updates={pages_after}");

    // ---- recovery: crash after the load, reopen (durable; setup is not timed) ----------------
    let recovery = measure_with_setup(
        &durable_plan,
        || -> TempDir {
            let dir = context.tempdir().expect("data directory");
            {
                let db = Database::open(dir.path()).expect("open");
                load_entity(&db, 1, args.rows).expect("load");
            } // dropped without close(): everything since the last checkpoint is replayed
            dir
        },
        |dir| {
            let db = Database::open(dir.path()).expect("recover");
            black_box(db.get(RowId::compose(1, 0)).expect("read"));
            (db, dir)
        },
    );
    let recovery_stats = SampleStats::from_measured(&recovery);
    let recovery_ms = recovery_stats.median_ns / 1e6;
    println!("recovery rows={} ms={recovery_ms:.1}", args.rows);
    report.set("recovery_ms", json!(recovery_ms));
    report.set(
        "recovery",
        report.case(
            &recovery_stats,
            object(vec![("rows", json!(args.rows)), ("ms", json!(recovery_ms))]),
        ),
    );

    let probe_after = context.probe(probe_dir.path());
    if let (Some(before), Some(after)) = (&probe_before, &probe_after) {
        report.set(
            "fsync_probe",
            json!({"before": before.to_json(), "after": after.to_json(), "drift_percent": before.drift_percent(after)}),
        );
    }

    // ---- relational execution (2.1) ----------------------------------------------------------
    let relational = relational(&args, &context, &report)?;
    report.set("relational", relational);
    report.set(
        "churn_heap_pages",
        json!({"before": pages_before, "after": pages_after}),
    );
    report.set(
        "host",
        json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cpus": thread::available_parallelism().map_or(0, usize::from),
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        }),
    );
    report.set("rows", json!(args.rows));
    report.set("iters", json!(args.iters));
    report.set("threads", json!(args.threads));

    if let Some(path) = args.output {
        let document = report.finish();
        let problems = adb_bench::report::validate(&document);
        if !problems.is_empty() {
            return Err(format!("the report breaks its own schema: {problems:?}").into());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&document)?)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
