//! The `scaling` and `recovery` families (Milestone 2.5.4.1).
//!
//! * **scaling** - how throughput changes with the number of threads (1, 2, 4, 8, 16): concurrent
//!   point lookups (CPU-bound; measured with the release plan) and concurrent single-row commits
//!   (fsync-bound; measured with the durable plan, a fixed total of commits split among the
//!   threads so every thread count does the same work). For each count the report gives the rate,
//!   the speed-up over one thread and the efficiency (speed-up divided by threads). More threads
//!   than logical CPUs measure the scheduler as much as the engine, and the report records the
//!   CPU count so a reader can tell.
//! * **recovery** - the time to open a database after a crash (everything since the last
//!   checkpoint is replayed) and after a clean close, for growing row counts.
//!
//! ```text
//! cargo run --release -p adb-benchmark-rust --bin scaling -- --family all --output out.json
//! ```

use std::{env, path::Path, path::PathBuf, sync::Arc, thread};

use adb_bench::report::{case_json, plan_json, rate_per_second};
use adb_bench::stats::DEFAULT_STABLE_BATCH_MAD_PERCENT;
use adb_bench::{measure, measure_with_setup, Context, Report, SampleStats};
use adb_core::{Row, RowId, Value};
use adb_engine::Database;
use serde_json::{json, Map, Value as Json};
use tempfile::TempDir;

/// Result type of the benchmark (any error aborts it).
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Thread counts of the scaling cases.
const THREADS: [u64; 5] = [1, 2, 4, 8, 16];

/// Lookups per thread in one invocation of the read-scaling case.
const LOOKUPS_PER_THREAD: u64 = 20_000;

/// Commits in one invocation of the commit-scaling case, split among the threads.
const COMMITS_TOTAL: u64 = 1_600;

/// Which families to run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Thread scaling only.
    Scaling,
    /// Recovery only.
    Recovery,
    /// Both.
    All,
}

/// Command-line options.
struct Args {
    /// Families to run.
    family: Family,
    /// Rows loaded for the read-scaling case.
    rows: u64,
    /// Row counts of the recovery case.
    recovery_rows: Vec<u64>,
    /// Report path.
    output: Option<PathBuf>,
    /// Directory durable cases write to.
    data_dir: Option<PathBuf>,
}

/// Parses `--family`, `--rows`, `--recovery-rows A,B,C`, `--output` and `--data-dir`.
fn parse_args() -> Result<Args> {
    let mut args = Args {
        family: Family::All,
        rows: 5_000,
        recovery_rows: vec![1_000, 5_000, 20_000],
        output: None,
        data_dir: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    for pair in raw.chunks(2) {
        match (pair[0].as_str(), pair.get(1)) {
            ("--family", Some(v)) => {
                args.family = match v.as_str() {
                    "scaling" => Family::Scaling,
                    "recovery" => Family::Recovery,
                    "all" => Family::All,
                    other => return Err(format!("unknown family {other}").into()),
                }
            }
            ("--rows", Some(v)) => args.rows = v.parse()?,
            ("--recovery-rows", Some(v)) => {
                args.recovery_rows = v
                    .split(',')
                    .map(|n| n.trim().parse::<u64>())
                    .collect::<std::result::Result<_, _>>()?;
            }
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

/// Loads `rows` rows of entity 1, 100 per transaction.
fn load(db: &Database, rows: u64) {
    for base in (0..rows).step_by(100) {
        let mut tx = db.begin();
        for pk in base..(base + 100).min(rows) {
            tx.put(RowId::compose(1, pk), row(pk));
        }
        db.commit(tx).expect("commit");
    }
}

/// Total size in bytes of the files below `dir`.
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_bytes(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Adds `speedup` and `efficiency` relative to the one-thread rate to each scaling entry.
fn relative_to_one_thread(rates: &[(u64, f64)], index: usize) -> (f64, f64) {
    let (threads, rate) = rates[index];
    let one = rates.first().map_or(0.0, |(_, r)| *r);
    if one > 0.0 {
        let speedup = rate / one;
        (speedup, speedup / threads as f64)
    } else {
        (0.0, 0.0)
    }
}

/// Concurrent point lookups and concurrent single-row commits for each thread count.
fn scaling(args: &Args, context: &Context) -> Result<Json> {
    let cpu = context.plan(false);
    // a single-thread invocation is COMMITS_TOTAL commits: seconds on a disk with a slow fsync
    let durable = context.heavy_plan();
    let mut out = Map::new();

    // ---- reads --------------------------------------------------------------------------------
    let dir = context.tempdir()?;
    let db = Arc::new(Database::open(dir.path())?);
    load(&db, args.rows);
    let mut cases = Vec::new();
    let mut rates = Vec::new();
    for &threads in &THREADS {
        let measured = measure(&cpu, || {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let db = Arc::clone(&db);
                    let rows = args.rows;
                    thread::spawn(move || {
                        let mut key = t * 101;
                        for _ in 0..LOOKUPS_PER_THREAD {
                            key = (key + 7919) % rows;
                            std::hint::black_box(db.get(RowId::compose(1, key)).expect("lookup"));
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("reader");
            }
        });
        let stats = SampleStats::from_measured(&measured);
        let rate = rate_per_second(&stats, (threads * LOOKUPS_PER_THREAD) as f64);
        rates.push((threads, rate));
        cases.push((threads, stats, rate));
    }
    let mut reads = Map::new();
    for (index, (threads, stats, rate)) in cases.iter().enumerate() {
        let (speedup, efficiency) = relative_to_one_thread(&rates, index);
        println!("reads threads={threads} lookups_s={rate:.0} speedup={speedup:.2}");
        reads.insert(
            format!("{threads}_threads"),
            with_extra(
                stats,
                vec![
                    ("threads", json!(threads)),
                    ("lookups_per_s", json!(rate)),
                    ("speedup", json!(speedup)),
                    ("efficiency", json!(efficiency)),
                ],
            ),
        );
    }
    out.insert("point_lookups".into(), Json::Object(reads));
    drop(db);
    drop(dir);

    // ---- commits --------------------------------------------------------------------------------
    let mut cases = Vec::new();
    let mut rates = Vec::new();
    for &threads in &THREADS {
        let per_thread = COMMITS_TOTAL / threads;
        let total = per_thread * threads;
        let measured = measure(&durable, || -> (TempDir, Arc<Database>) {
            let dir = context.tempdir().expect("data directory");
            let db = Arc::new(Database::open(dir.path()).expect("open"));
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let db = Arc::clone(&db);
                    thread::spawn(move || {
                        for pk in 0..per_thread {
                            let mut tx = db.begin();
                            tx.put(RowId::compose(t, pk), row(pk));
                            db.commit(tx).expect("commit");
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("writer");
            }
            (dir, db)
        });
        let stats = SampleStats::from_measured(&measured);
        let rate = rate_per_second(&stats, total as f64);
        rates.push((threads, rate));
        cases.push((threads, stats, rate));
    }
    let mut commits = Map::new();
    for (index, (threads, stats, rate)) in cases.iter().enumerate() {
        let (speedup, efficiency) = relative_to_one_thread(&rates, index);
        println!("commits threads={threads} commits_s={rate:.0} speedup={speedup:.2}");
        commits.insert(
            format!("{threads}_threads"),
            with_extra(
                stats,
                vec![
                    ("threads", json!(threads)),
                    ("commits_per_s", json!(rate)),
                    ("speedup", json!(speedup)),
                    ("efficiency", json!(efficiency)),
                ],
            ),
        );
    }
    out.insert("single_row_commits".into(), Json::Object(commits));
    out.insert(
        "setup".into(),
        json!({"rows": args.rows, "lookups_per_thread": LOOKUPS_PER_THREAD,
               "commits_total": COMMITS_TOTAL, "threads": THREADS,
               "logical_cpus": context.environment.logical_cpus}),
    );
    Ok(Json::Object(out))
}

/// The JSON of one case with `extra` keys, using the default stability limit.
fn with_extra(stats: &SampleStats, extra: Vec<(&str, Json)>) -> Json {
    let extra = extra.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    case_json(stats, DEFAULT_STABLE_BATCH_MAD_PERCENT, extra)
}

/// Time to open a database after a crash and after a clean close, per row count.
fn recovery(args: &Args, context: &Context) -> Result<Json> {
    let plan = context.plan(true);
    let mut out = Map::new();
    for &rows in &args.recovery_rows {
        for (mode, clean) in [("after_crash", false), ("after_clean_close", true)] {
            let measured = measure_with_setup(
                &plan,
                || -> (TempDir, u64) {
                    let dir = context.tempdir().expect("data directory");
                    {
                        let db = Database::open(dir.path()).expect("open");
                        load(&db, rows);
                        if clean {
                            db.close().expect("close");
                        }
                        // otherwise dropped without close(): the log since the last checkpoint is replayed
                    }
                    let bytes = dir_bytes(dir.path());
                    (dir, bytes)
                },
                |(dir, bytes)| {
                    let db = Database::open(dir.path()).expect("recover");
                    std::hint::black_box(db.get(RowId::compose(1, 0)).expect("read"));
                    (dir, db, bytes)
                },
            );
            let stats = SampleStats::from_measured(&measured);
            let ms = stats.median_ns / 1e6;
            let bytes = measured.last.2;
            println!("recovery rows={rows} {mode} ms={ms:.1} data_bytes={bytes}");
            out.insert(
                format!("{rows}_rows_{mode}"),
                with_extra(
                    &stats,
                    vec![
                        ("rows", json!(rows)),
                        ("mode", json!(mode)),
                        ("ms", json!(ms)),
                        ("rows_per_s", json!(rate_per_second(&stats, rows as f64))),
                        ("data_bytes", json!(bytes)),
                    ],
                ),
            );
        }
    }
    Ok(Json::Object(out))
}

/// Runs the chosen families and writes the report.
fn main() -> Result<()> {
    let args = parse_args()?;
    let context = Context::new(args.data_dir.clone());
    let cpu = context.plan(false);
    let mut report = Report::new("scaling", "2.5.4.1", &cpu, &context.environment);
    report.set("plan_durable", plan_json(&context.plan(true)));
    report.set("plan_heavy", plan_json(&context.heavy_plan()));
    report.set("storage", context.storage().to_json());
    let probe_dir = context.tempdir()?;
    let probe_before = context.probe(probe_dir.path());
    println!("Adaptive DB 2.5.4.1 scaling/recovery benchmark");
    if args.family != Family::Recovery {
        report.set("scaling", scaling(&args, &context)?);
    }
    if args.family != Family::Scaling {
        report.set("recovery", recovery(&args, &context)?);
    }
    if let (Some(before), Some(after)) = (&probe_before, &context.probe(probe_dir.path())) {
        report.set(
            "fsync_probe",
            json!({"before": before.to_json(), "after": after.to_json(), "drift_percent": before.drift_percent(after)}),
        );
    }
    let document = report.finish();
    let problems = adb_bench::report::validate(&document);
    if !problems.is_empty() {
        return Err(format!("the report breaks its own schema: {problems:?}").into());
    }
    if let Some(path) = args.output {
        std::fs::write(&path, serde_json::to_string_pretty(&document)? + "\n")?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
