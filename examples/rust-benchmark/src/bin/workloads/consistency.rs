//! Milestone 2.5.0 workloads: what the global commit order, the change log and the projection
//! runtime cost, measured in Rust on databases of their own.
//!
//! * **I_commit_with_change_log** — commit throughput with the change log on (it always is), for
//!   one row per commit and for 100 rows per commit, and the log bytes each commit adds. Compare
//!   with the 2.4.0 `write_overhead` section of an earlier report for the cost of the log;
//! * **J_change_log_stream** — a cursor reading the whole log in batches: commits and MiB per
//!   second, and a checksum (changes seen) that must equal the rows written;
//! * **K_cursor_start** — time to open a cursor and read its first batch at the start, the middle
//!   and the end of a multi-segment log;
//! * **L_projection_wait_latency** — a consumer thread follows the log while the main thread
//!   commits and then waits for the projection to reach the commit: the latency distribution;
//! * **M_projection_catch_up** — a consumer fed a backlog of commits: commits per second and the
//!   lag afterwards (must be 0);
//! * **N_recovery_tail_rebuild** — opening a database with an intact change log, and with the log
//!   deleted so that it is rebuilt from the WAL;
//! * **O_retention_prune** — pruning a multi-segment log down to a byte limit: segments and bytes
//!   freed and the time it took.

use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use crate::harness::tempdir;
use adb_change_log::{ChangePayload, ChangeRetentionPolicy};
use adb_core::{CommitSeq, Row, RowId, Value};
use adb_engine::{Database, DatabaseOptions};
use adb_projection_runtime::WaitOutcome;
use serde_json::json;

use crate::harness::{case, cpu_plan, durable_plan, heavy_plan};
use crate::Result;
use adb_bench::SampleStats;

/// Commits of the throughput and streaming measurements at scale factor 1.
const BASE_COMMITS: u64 = 5_000;
/// Commits per invocation of the throughput measurement (I) at scale factor 1. Its result is a
/// rate, so it needs far fewer commits than the log the streaming workloads read: at a few
/// milliseconds per `fsync`, 5 000 commits took 40 to 55 s per invocation.
const THROUGHPUT_COMMITS: u64 = 1_000;
/// Rows per commit of the batched throughput measurement.
const BATCH_ROWS: u64 = 100;
/// Commits per batch a consumer asks for.
const FEED_COMMITS: usize = 500;
/// Change-log segment size of the multi-segment measurements: small, so the log rolls over.
const SMALL_SEGMENT_BYTES: u64 = 256 * 1024;

/// A row with a 64-byte payload.
fn payload(n: u64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(n as i64))
        .with_field(2, Value::String("x".repeat(64)))
}

/// Commits `commits` transactions of `rows` puts each, starting at row `first`; returns the next
/// free row number.
fn load(db: &Database, first: u64, commits: u64, rows: u64) -> Result<u64> {
    let mut next = first;
    for _ in 0..commits {
        let mut tx = db.begin();
        for _ in 0..rows {
            tx.put(RowId(next as u128), payload(next));
            next += 1;
        }
        db.commit(tx)?;
    }
    Ok(next)
}

/// Opens a database whose change log rolls over every [`SMALL_SEGMENT_BYTES`].
fn open_small(path: &Path) -> Result<Database> {
    Ok(Database::open_with(
        path,
        DatabaseOptions {
            change_log_segment_bytes: SMALL_SEGMENT_BYTES,
            ..Default::default()
        },
    )?)
}

/// Commit throughput and log growth for one row and for [`BATCH_ROWS`] rows per commit, with the
/// heavy plan (each invocation loads `commits` commits into a fresh database).
fn commit_throughput(commits: u64) -> Result<serde_json::Value> {
    let mut report = serde_json::Map::new();
    for (name, rows) in [("one_row_per_commit", 1), ("batched", BATCH_ROWS)] {
        eprintln!("workloads: I_commit_with_change_log {name} ({commits} commits per invocation)");
        let measured = adb_bench::measure(&heavy_plan(), || {
            let dir = tempdir().expect("data directory");
            let db = Database::open(dir.path()).expect("open");
            let before = db.change_log().total_bytes();
            load(&db, 1, commits, rows).expect("commits");
            let bytes = db.change_log().total_bytes() - before;
            (dir, db, bytes)
        });
        let stats = SampleStats::from_measured(&measured);
        let bytes = measured.last.2;
        let seconds = stats.median_ns / 1e9;
        report.insert(
            name.into(),
            case(
                &stats,
                vec![
                    ("commits", json!(commits)),
                    ("rows_per_commit", json!(rows)),
                    ("seconds", json!(seconds)),
                    ("commits_per_s", json!(commits as f64 / seconds)),
                    ("rows_per_s", json!((commits * rows) as f64 / seconds)),
                    (
                        "change_log_bytes_per_commit",
                        json!(bytes as f64 / commits as f64),
                    ),
                ],
            ),
        );
    }
    Ok(serde_json::Value::Object(report))
}

/// Streams the whole log through a cursor; the checksum is the number of row changes seen.
fn stream(db: &Database, expected_changes: u64) -> Result<serde_json::Value> {
    let mut cursor = db.change_log().cursor(CommitSeq::ZERO)?;
    let (mut commits, mut changes, mut bytes) = (0u64, 0u64, 0u64);
    let start = Instant::now();
    while let Some(batch) = cursor.next_batch(FEED_COMMITS, 4 << 20)? {
        commits += batch.commits.len() as u64;
        bytes += batch.bytes;
        for commit in &batch.commits {
            if let ChangePayload::Data { rows, .. } = &commit.payload {
                changes += rows.len() as u64;
            }
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(
        json!({"commits": commits, "seconds": seconds, "commits_per_s": commits as f64 / seconds,
              "mib_per_s": bytes as f64 / (1 << 20) as f64 / seconds,
              "checksum": changes, "expected_changes": expected_changes}),
    )
}

/// Time to open a cursor and read its first batch after `position`, with the CPU-bound plan.
fn cursor_start(db: &Database, position: CommitSeq) -> Result<serde_json::Value> {
    let measured = adb_bench::measure(&cpu_plan(), || {
        let mut cursor = db.change_log().cursor(position).expect("cursor");
        let batch = cursor.next_batch(FEED_COMMITS, 4 << 20).expect("batch");
        batch.map_or(0, |b| b.first_commit_seq.0)
    });
    let stats = SampleStats::from_measured(&measured);
    Ok(case(
        &stats,
        vec![("first_commit_seq", json!(measured.last))],
    ))
}

/// Latency from a commit to its asynchronous projection reaching it.
fn wait_latency() -> Result<serde_json::Value> {
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let id = db.register_consumer_projection("bench", None)?;
    let feeder = db.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    let consumer = thread::spawn(move || {
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            let applied = feeder
                .feed_projection(id, (FEED_COMMITS, u64::MAX), |_| Ok(()))
                .unwrap_or(0);
            if applied == 0 {
                thread::sleep(Duration::from_micros(200));
            }
        }
    });
    // One invocation = one commit followed by one wait for the projection to reach it; the commit
    // is part of the invocation but is cheap next to the wait only when the consumer keeps up.
    // The wait is timed on its own inside the invocation, so the sample is the wait alone.
    let timed_out = std::cell::Cell::new(0u64);
    let next = std::cell::Cell::new(1u64);
    let measured = adb_bench::measure_with_setup(
        &durable_plan(),
        || {
            next.set(load(&db, next.get(), 1, 1).expect("commit"));
            db.latest_commit_seq()
        },
        |target| {
            let outcome = db
                .projection_registry()
                .wait_until(id, target, Duration::from_secs(5));
            if outcome != WaitOutcome::Reached {
                timed_out.set(timed_out.get() + 1);
            }
        },
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    consumer.join().expect("consumer thread");
    let stats = SampleStats::from_measured(&measured);
    let mut report = case(
        &stats,
        vec![
            ("stability_skipped_reason", json!(WAIT_LATENCY_NOT_JUDGED)),
            (
                "waits",
                json!(durable_plan().total_samples() + durable_plan().warmup_invocations + 1),
            ),
            ("timed_out", json!(timed_out.get())),
        ],
    );
    // the batch medians are reported, but their spread is the poll interval, not the engine
    if let Some(timing) = report.get_mut("timing") {
        timing["stability"] = json!("skipped");
    }
    db.close()?;
    Ok(report)
}

/// Why the stability of L is not judged: the consumer sleeps 200 µs whenever it finds nothing new,
/// so a wait lasts anywhere between the apply time and the apply time plus one sleep. Batch MADs of
/// 18 to 50 percent on the reference machine are that sleep, not noise.
const WAIT_LATENCY_NOT_JUDGED: &str =
    "the wait includes up to one 200 us consumer poll interval, so its spread is the poll, not noise";

/// A consumer fed a backlog of `commits` commits.
fn catch_up(commits: u64) -> Result<serde_json::Value> {
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    load(&db, 1, commits, 1)?;
    let id = db.register_consumer_projection("bench", None)?;
    let start = Instant::now();
    let mut applied = 0;
    loop {
        let n = db.feed_projection(id, (FEED_COMMITS, u64::MAX), |_| Ok(()))?;
        if n == 0 {
            break;
        }
        applied += n;
    }
    let seconds = start.elapsed().as_secs_f64();
    let lag = db
        .projection_registry()
        .get(id)
        .map_or(u64::MAX, |p| p.lag_commits);
    db.close()?;
    Ok(
        json!({"backlog_commits": commits, "applied": applied, "seconds": seconds,
              "commits_per_s": applied as f64 / seconds, "lag_after": lag}),
    )
}

/// Open time with an intact change log, and with the log deleted (rebuilt from the WAL).
fn recovery(commits: u64) -> Result<serde_json::Value> {
    let dir = tempdir()?;
    let head;
    {
        let db = Database::open(dir.path())?;
        load(&db, 1, commits, 1)?;
        head = db.latest_commit_seq();
        db.close()?;
    }
    let start = Instant::now();
    let db = Database::open(dir.path())?;
    let intact = start.elapsed().as_secs_f64() * 1e3;
    let intact_head = db.change_log().head();
    db.close()?;
    std::fs::remove_dir_all(dir.path().join("change-log"))?;
    let start = Instant::now();
    let db = Database::open(dir.path())?;
    let rebuilt = start.elapsed().as_secs_f64() * 1e3;
    let rebuilt_head = db.change_log().head();
    db.close()?;
    Ok(
        json!({"commits": commits, "open_intact_ms": intact, "open_rebuilt_ms": rebuilt,
              "head": head.0, "intact_log_head": intact_head.map(|h| h.0),
              "rebuilt_log_head": rebuilt_head.map(|h| h.0)}),
    )
}

/// Pruning a multi-segment log, once after a consumer has read everything (the frontier frees
/// the segments) and once with a consumer that has read nothing (only the byte limit can free
/// them, which overtakes the consumer).
fn prune(commits: u64) -> Result<serde_json::Value> {
    let mut report = serde_json::Map::new();
    for (name, consumed) in [
        ("consumer_caught_up", true),
        ("slow_consumer_byte_limit", false),
    ] {
        let dir = tempdir()?;
        let db = open_small(dir.path())?;
        load(&db, 1, commits, 10)?;
        let id = db.register_consumer_projection("bench", None)?;
        if consumed {
            while db.feed_projection(id, (FEED_COMMITS, u64::MAX), |_| Ok(()))? > 0 {}
        }
        let before = db.change_log().total_bytes();
        let segments_before = db.change_log().segment_count();
        let start = Instant::now();
        let pruned = db.prune_change_log(&ChangeRetentionPolicy {
            max_bytes: Some(before / 2),
            ..Default::default()
        })?;
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let state = db
            .projection_registry()
            .get(id)
            .map(|p| format!("{:?}", p.state));
        report.insert(
            name.into(),
            json!({"bytes_before": before, "bytes_after": db.change_log().total_bytes(),
                   "segments_before": segments_before, "segments_after": db.change_log().segment_count(),
                   "segments_removed": pruned.segments_removed, "bytes_freed": pruned.bytes_freed,
                   "overtook_frontier": pruned.overtook_frontier, "consumer_state": state, "ms": ms}),
        );
        db.close()?;
    }
    Ok(serde_json::Value::Object(report))
}

/// Runs every 2.5.0 workload; `scale` multiplies the commit counts.
pub fn report(scale: u64) -> Result<serde_json::Value> {
    let commits = BASE_COMMITS * scale;
    let rows_per_commit = 10;
    let dir = tempdir()?;
    let db = open_small(dir.path())?;
    load(&db, 1, commits, rows_per_commit)?;
    let head = db.latest_commit_seq();
    let streaming = stream(&db, commits * rows_per_commit)?;
    let starts = json!({
        "start": cursor_start(&db, CommitSeq::ZERO)?,
        "middle": cursor_start(&db, CommitSeq(head.0 / 2))?,
        "end": cursor_start(&db, CommitSeq(head.0.saturating_sub(1)))?,
        "segments": db.change_log().segment_count(),
    });
    db.close()?;
    Ok(json!({
        "setup": {"commits": commits, "rows_per_commit": rows_per_commit, "segment_bytes": SMALL_SEGMENT_BYTES},
        "workloads": {
            "I_commit_with_change_log": commit_throughput(THROUGHPUT_COMMITS * scale)?,
            "J_change_log_stream": streaming,
            "K_cursor_start": starts,
            "L_projection_wait_latency": wait_latency()?,
            "M_projection_catch_up": catch_up(commits)?,
            "N_recovery_tail_rebuild": recovery(commits)?,
            "O_retention_prune": prune(commits)?,
        },
    }))
}
