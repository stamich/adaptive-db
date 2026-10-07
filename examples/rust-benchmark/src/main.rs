//! Milestone 2.0.3 data-plane benchmark.
//!
//! Sections comparable with 2.0.2 (bulk load, point lookup, scan, plan parsing) plus the
//! behaviours introduced in 2.0.3: single-row commit throughput with and without concurrency
//! (group commit), heap size under update churn, entity scan, change-feed throughput and
//! recovery time.

use std::{
    env,
    hint::black_box,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use adb_core::{Row, RowId, Value};
use adb_engine::{ChangeCursor, ChangeFilter, Database};
use adb_execution::{BinaryOp, Expr, PhysicalPlan};
use serde_json::json;
use tempfile::tempdir;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Args {
    rows: u64,
    iters: usize,
    threads: u64,
    output: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        rows: 5_000,
        iters: 1_000,
        threads: 8,
        output: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        let value = raw.get(i + 1).ok_or("missing argument value")?;
        match raw[i].as_str() {
            "--rows" => args.rows = value.parse()?,
            "--iters" => args.iters = value.parse()?,
            "--threads" => args.threads = value.parse()?,
            "--output" => args.output = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument: {other}").into()),
        }
        i += 2;
    }
    Ok(args)
}

fn row(value: u64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(value as i64))
        .with_field(2, Value::String(format!("row-{value}")))
}

fn per_second(count: u64, elapsed: Duration) -> f64 {
    count as f64 / elapsed.as_secs_f64()
}

fn percentiles(mut samples: Vec<u128>) -> (u128, u128, u128) {
    samples.sort_unstable();
    let at = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    (at(0.50), at(0.95), at(0.99))
}

fn count_rows(db: &Database, plan: PhysicalPlan) -> Result<usize> {
    let mut cursor = db.execute(plan)?;
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch()? {
        rows += batch.len();
        black_box(&batch);
    }
    Ok(rows)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    println!("Adaptive DB 2.0.3 Rust benchmark");
    println!(
        "rows={} iters={} threads={}",
        args.rows, args.iters, args.threads
    );

    // ---- bulk load: 100 rows per transaction, 3 entities ------------------------------------
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let start = Instant::now();
    for entity in 1..=3u64 {
        for base in (0..args.rows).step_by(100) {
            let mut tx = db.begin();
            for pk in base..(base + 100).min(args.rows) {
                tx.put(RowId::compose(entity, pk), row(pk));
            }
            db.commit(tx)?;
        }
    }
    let load = start.elapsed();
    let loaded = 3 * args.rows;
    println!(
        "load rows={loaded} ms={} rows_s={:.0}",
        load.as_millis(),
        per_second(loaded, load)
    );

    // ---- point lookup -------------------------------------------------------------------------
    let mut key = 0u64;
    let mut samples = Vec::with_capacity(args.iters);
    let start = Instant::now();
    for _ in 0..args.iters {
        key = (key + 7919) % args.rows;
        let t = Instant::now();
        black_box(db.get(RowId::compose(2, key))?);
        samples.push(t.elapsed().as_nanos());
    }
    let lookup = start.elapsed();
    let (p50, p95, p99) = percentiles(samples);
    println!(
        "point_lookup ops_s={:.0} p50_ns={p50} p95_ns={p95} p99_ns={p99}",
        per_second(args.iters as u64, lookup)
    );

    // ---- entity scan vs full scan + filter (the 2.0.2 plan shape) -----------------------------
    let start = Instant::now();
    let entity_rows = count_rows(&db, PhysicalPlan::EntityScan { entity_id: 2 })?;
    let entity_scan = start.elapsed();
    let start = Instant::now();
    let filtered_rows = count_rows(
        &db,
        PhysicalPlan::Filter {
            input: Box::new(PhysicalPlan::Scan),
            predicate: Expr::Binary {
                left: Box::new(Expr::Column { field_id: 1 }),
                op: BinaryOp::Ge,
                right: Box::new(Expr::Literal {
                    value: Value::Int64(0),
                }),
            },
        },
    )?;
    let full_scan = start.elapsed();
    println!(
        "entity_scan rows={entity_rows} ms={:.2} | full_scan+filter rows={filtered_rows} ms={:.2}",
        entity_scan.as_secs_f64() * 1e3,
        full_scan.as_secs_f64() * 1e3
    );

    // ---- plan JSON parsing ----------------------------------------------------------------------
    let plan_json = serde_json::to_string(&PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::EntityScan { entity_id: 2 }),
        limit: 100,
    })?;
    let start = Instant::now();
    for _ in 0..args.iters {
        black_box(serde_json::from_str::<PhysicalPlan>(&plan_json)?);
    }
    let parse = per_second(args.iters as u64, start.elapsed());
    println!("physical_plan_json_parse ops_s={parse:.0}");

    // ---- change feed ----------------------------------------------------------------------------
    let start = Instant::now();
    let mut cursor = ChangeCursor::BEGINNING;
    let mut events = 0u64;
    loop {
        let batch = db.read_changes(cursor, 1_000, &ChangeFilter::all())?;
        if batch.events.is_empty() {
            break;
        }
        events += batch.events.len() as u64;
        cursor = batch.next;
    }
    let feed = start.elapsed();
    println!(
        "cdc_read events={events} ms={} events_s={:.0}",
        feed.as_millis(),
        per_second(events, feed)
    );

    // ---- single-row commits: 1 thread vs N threads (group commit) ----------------------------
    let single_n = args.rows.min(2_000);
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let start = Instant::now();
    for pk in 0..single_n {
        let mut tx = db.begin();
        tx.put(RowId::compose(1, pk), row(pk));
        db.commit(tx)?;
    }
    let commit_1 = per_second(single_n, start.elapsed());

    let dir = tempdir()?;
    let db = Arc::new(Database::open(dir.path())?);
    let per_thread = single_n / args.threads.max(1);
    let start = Instant::now();
    let handles: Vec<_> = (0..args.threads)
        .map(|t| {
            let db = Arc::clone(&db);
            thread::spawn(move || -> std::result::Result<(), String> {
                for pk in 0..per_thread {
                    let mut tx = db.begin();
                    tx.put(RowId::compose(t, pk), row(pk));
                    db.commit(tx).map_err(|e| e.to_string())?;
                }
                Ok(())
            })
        })
        .collect();
    for handle in handles {
        handle.join().map_err(|_| "worker panicked")??;
    }
    let commit_n = per_second(per_thread * args.threads, start.elapsed());
    println!(
        "commit_single_row 1_thread_ops_s={commit_1:.0} {}_threads_ops_s={commit_n:.0}",
        args.threads
    );

    // ---- update churn: current heap must stay bounded ------------------------------------------
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let mut tx = db.begin();
    for pk in 0..100 {
        tx.put(RowId::compose(1, pk), row(0));
    }
    db.commit(tx)?;
    let pages_before = db.storage_stats()?.current_heap_pages;
    for round in 0..20 {
        let mut tx = db.begin();
        for pk in 0..100 {
            tx.put(RowId::compose(1, pk), row(round));
        }
        db.commit(tx)?;
    }
    let pages_after = db.storage_stats()?.current_heap_pages;
    println!("churn_heap_pages before={pages_before} after_2000_updates={pages_after}");

    // ---- recovery: crash after the load, reopen ------------------------------------------------
    let dir = tempdir()?;
    {
        let db = Database::open(dir.path())?;
        for base in (0..args.rows).step_by(100) {
            let mut tx = db.begin();
            for pk in base..(base + 100).min(args.rows) {
                tx.put(RowId::compose(1, pk), row(pk));
            }
            db.commit(tx)?;
        }
    } // dropped without close(): everything since the last checkpoint is replayed
    let start = Instant::now();
    let db = Database::open(dir.path())?;
    let recovery = start.elapsed();
    black_box(db.get(RowId::compose(1, 0))?);
    println!("recovery rows={} ms={}", args.rows, recovery.as_millis());

    if let Some(path) = args.output {
        let report = json!({
            "milestone": "2.0.3",
            "rows": args.rows, "iters": args.iters, "threads": args.threads,
            "load": {"rows": loaded, "ms": load.as_millis(), "rows_s": per_second(loaded, load)},
            "point_lookup": {"ops_s": per_second(args.iters as u64, lookup),
                              "p50_ns": p50, "p95_ns": p95, "p99_ns": p99},
            "entity_scan": {"rows": entity_rows, "ms": entity_scan.as_secs_f64() * 1e3},
            "full_scan_filter": {"rows": filtered_rows, "ms": full_scan.as_secs_f64() * 1e3},
            "plan_json_parse_ops_s": parse,
            "cdc_read": {"events": events, "ms": feed.as_millis()},
            "commit_single_row": {"one_thread_ops_s": commit_1, "n_threads_ops_s": commit_n},
            "churn_heap_pages": {"before": pages_before, "after": pages_after},
            "recovery_ms": recovery.as_millis(),
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&report)?)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
