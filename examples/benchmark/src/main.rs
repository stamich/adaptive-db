//! Rust-only baseline benchmark for Adaptive DB Milestone 1.5.2.
//!
//! This is an intentionally dependency-light benchmark harness. It is meant to
//! produce repeatable milestone-to-milestone baseline data, not to replace a
//! statistical microbenchmark framework such as Criterion.

use std::{
    env, fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use adb_btree::BTree;
use adb_core::{PageId, Row, RowId, RowLocation, Value};
use adb_engine::Database;
use tempfile::tempdir;

#[derive(Debug, Clone)]
struct Config {
    rows: usize,
    batch_size: usize,
    lookups: usize,
    historical_versions: usize,
    output: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rows: 10_000,
            batch_size: 100,
            lookups: 100_000,
            historical_versions: 1_000,
            output: None,
        }
    }
}

#[derive(Debug, Clone)]
struct Measurement {
    name: &'static str,
    operations: usize,
    elapsed: Duration,
}

impl Measurement {
    fn ops_per_sec(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.operations as f64 / self.elapsed.as_secs_f64()
    }

    fn ns_per_op(&self) -> f64 {
        if self.operations == 0 {
            return 0.0;
        }
        self.elapsed.as_nanos() as f64 / self.operations as f64
    }
}

fn row(v: i64) -> Row {
    Row::new().with_field(1, Value::Int64(v))
}

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = |args: &mut std::iter::Skip<std::env::Args>, flag: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match arg.as_str() {
            "--rows" => {
                cfg.rows = value(&mut args, "--rows")?
                    .parse()
                    .map_err(|_| "invalid --rows")?
            }
            "--batch-size" => {
                cfg.batch_size = value(&mut args, "--batch-size")?
                    .parse()
                    .map_err(|_| "invalid --batch-size")?
            }
            "--lookups" => {
                cfg.lookups = value(&mut args, "--lookups")?
                    .parse()
                    .map_err(|_| "invalid --lookups")?
            }
            "--historical-versions" => {
                cfg.historical_versions = value(&mut args, "--historical-versions")?
                    .parse()
                    .map_err(|_| "invalid --historical-versions")?
            }
            "--output" => cfg.output = Some(PathBuf::from(value(&mut args, "--output")?)),
            "--help" | "-h" => {
                println!("adb-benchmark-1-5-2 [--rows N] [--batch-size N] [--lookups N] [--historical-versions N] [--output FILE]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if cfg.rows == 0 || cfg.batch_size == 0 || cfg.lookups == 0 || cfg.historical_versions == 0 {
        return Err("numeric arguments must be greater than zero".into());
    }
    Ok(cfg)
}

fn measure(name: &'static str, operations: usize, f: impl FnOnce()) -> Measurement {
    let start = Instant::now();
    f();
    Measurement {
        name,
        operations,
        elapsed: start.elapsed(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = parse_args().map_err(|e| format!("argument error: {e}"))?;
    println!("Adaptive DB 1.5.2 Rust baseline benchmark");
    println!(
        "rows={} batch_size={} lookups={} historical_versions={}",
        cfg.rows, cfg.batch_size, cfg.lookups, cfg.historical_versions
    );
    println!(
        "NOTE: durable commit numbers include WAL sync + Current flush + checkpoint publication.\n"
    );

    let dir = tempdir()?;
    let mut measurements = Vec::new();

    // B01: B+Tree physical index insertion.
    let tree_path = dir.path().join("btree");
    fs::create_dir_all(&tree_path)?;
    let tree = BTree::open(
        tree_path.join("index.dat"),
        tree_path.join("index.meta"),
        128,
    )?;
    measurements.push(measure("btree_insert", cfg.rows, || {
        for i in 0..cfg.rows {
            tree.insert(
                RowId(i as u64 as u128),
                RowLocation {
                    page_id: PageId((i / 8) as u64),
                    slot_id: (i % 8) as u16,
                },
            )
                .unwrap();
        }
        tree.flush().unwrap();
    }));

    // B02: B+Tree point lookups (warm buffer cache after insert).
    measurements.push(measure("btree_get_warm", cfg.lookups, || {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..cfg.lookups {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let id = (state as usize % cfg.rows) as u64;
            black_box(tree.get(RowId(id as u128)).unwrap());
        }
    }));

    // B03: one transaction per row: intentionally measures full durability path.
    let single_dir = dir.path().join("single-commit-db");
    let single_db = Database::open(&single_dir)?;
    measurements.push(measure(
        "durable_single_row_commit",
        cfg.rows.min(2_000),
        || {
            for i in 0..cfg.rows.min(2_000) {
                let mut tx = single_db.begin();
                tx.put(RowId(i as u64 as u128), row(i as i64));
                black_box(single_db.commit(tx).unwrap());
            }
        },
    ));

    // B04: batched durable transactions, amortizing WAL/checkpoint cost.
    let batch_dir = dir.path().join("batched-db");
    let batch_db = Database::open(&batch_dir)?;
    measurements.push(measure("durable_batched_insert", cfg.rows, || {
        let mut base = 0usize;
        while base < cfg.rows {
            let end = (base + cfg.batch_size).min(cfg.rows);
            let mut tx = batch_db.begin();
            for i in base..end {
                tx.put(RowId(i as u64 as u128), row(i as i64));
            }
            black_box(batch_db.commit(tx).unwrap());
            base = end;
        }
    }));

    // B05: Current-store lookups through the complete Database API.
    measurements.push(measure("database_current_get", cfg.lookups, || {
        let mut state = 0xd1b5_4a32_d192_ed03u64;
        for _ in 0..cfg.lookups {
            state = state
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            let id = (state as usize % cfg.rows) as u64;
            black_box(batch_db.get(RowId(id as u128)).unwrap());
        }
    }));

    // B06: Historical lookup: current version is newer than requested snapshot,
    // forcing VersionStore participation.
    let history_dir = dir.path().join("history-db");
    let history_db = Database::open(&history_dir)?;
    let mut first = history_db.begin();
    for i in 0..cfg.historical_versions {
        first.put(RowId(i as u64 as u128), row(i as i64));
    }
    let old_ts = history_db.commit(first)?;
    let mut second = history_db.begin();
    for i in 0..cfg.historical_versions {
        second.put(RowId(i as u64 as u128), row(i as i64 + 1_000_000));
    }
    history_db.commit(second)?;
    measurements.push(measure("historical_get_at", cfg.lookups, || {
        for i in 0..cfg.lookups {
            black_box(
                history_db
                    .get_at(RowId((i % cfg.historical_versions) as u64 as u128), old_ts)
                    .unwrap(),
            );
        }
    }));

    // B07: clean reopen/recovery. Historical versions are reconstructed from WAL;
    // Current is read from persistent heap+B+Tree and checkpoint metadata is validated.
    drop(history_db);
    measurements.push(measure("database_reopen_recovery", 1, || {
        let reopened = Database::open(&history_dir).unwrap();
        black_box(reopened.get(RowId(0)).unwrap());
        black_box(reopened.get_at(RowId(0), old_ts).unwrap());
    }));

    println!(
        "{:<30} {:>12} {:>14} {:>14} {:>14}",
        "benchmark", "operations", "elapsed_ms", "ops/s", "ns/op"
    );
    for m in &measurements {
        println!(
            "{:<30} {:>12} {:>14.3} {:>14.1} {:>14.1}",
            m.name,
            m.operations,
            m.elapsed.as_secs_f64() * 1000.0,
            m.ops_per_sec(),
            m.ns_per_op()
        );
    }

    if let Some(path) = &cfg.output {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut json = String::from("{\n  \"milestone\": \"1.5.2\",\n  \"engine_base\": \"1.5.1-hardened\",\n  \"measurements\": [\n");
        for (i, m) in measurements.iter().enumerate() {
            let comma = if i + 1 == measurements.len() { "" } else { "," };
            json.push_str(&format!("    {{\"name\":\"{}\",\"operations\":{},\"elapsed_ns\":{},\"ops_per_sec\":{:.3},\"ns_per_op\":{:.3}}}{}\n",
                                   m.name, m.operations, m.elapsed.as_nanos(), m.ops_per_sec(), m.ns_per_op(), comma));
        }
        json.push_str("  ]\n}\n");
        fs::write(path, json)?;
        println!("\nJSON results written to {}", path.display());
    }

    Ok(())
}
