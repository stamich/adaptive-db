//! Rust-only baseline benchmark for Adaptive DB Milestone 1.6.2.
//!
//! The harness intentionally uses only the standard library for timing so the
//! milestone remains dependency-light. It is a comparative release baseline,
//! not a substitute for Criterion when statistically rigorous microbenchmarks
//! are required.

use std::{
    env, fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use adb_btree::VersionBTree;
use adb_core::{CommitTs, Lsn, PageId, Row, RowId, RowLocation, TxId, Value, VersionKey};
use adb_engine::Database;
use adb_storage::{HistoricalVersion, PersistentVersionStore};
use adb_wal::{SegmentedWalReader, SegmentedWalWriter, WalRecord};
use tempfile::tempdir;

#[derive(Debug, Clone)]
struct Config {
    rows: usize,
    batch_size: usize,
    lookups: usize,
    versions_per_row: usize,
    temporal_rows: usize,
    output: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rows: 10_000,
            batch_size: 100,
            lookups: 100_000,
            versions_per_row: 4,
            temporal_rows: 1_000,
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
            0.0
        } else {
            self.operations as f64 / self.elapsed.as_secs_f64()
        }
    }
    fn ns_per_op(&self) -> f64 {
        if self.operations == 0 {
            0.0
        } else {
            self.elapsed.as_nanos() as f64 / self.operations as f64
        }
    }
}

fn row(v: i64) -> Row {
    Row::new().with_field(1, Value::Int64(v))
}

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let next = |args: &mut std::iter::Skip<std::env::Args>, flag: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match arg.as_str() {
            "--rows" => {
                cfg.rows = next(&mut args, "--rows")?
                    .parse()
                    .map_err(|_| "invalid --rows")?
            }
            "--batch-size" => {
                cfg.batch_size = next(&mut args, "--batch-size")?
                    .parse()
                    .map_err(|_| "invalid --batch-size")?
            }
            "--lookups" => {
                cfg.lookups = next(&mut args, "--lookups")?
                    .parse()
                    .map_err(|_| "invalid --lookups")?
            }
            "--versions-per-row" => {
                cfg.versions_per_row = next(&mut args, "--versions-per-row")?
                    .parse()
                    .map_err(|_| "invalid --versions-per-row")?
            }
            "--temporal-rows" => {
                cfg.temporal_rows = next(&mut args, "--temporal-rows")?
                    .parse()
                    .map_err(|_| "invalid --temporal-rows")?
            }
            "--output" => cfg.output = Some(PathBuf::from(next(&mut args, "--output")?)),
            "--help" | "-h" => {
                println!("adb-benchmark-1-6-2 [--rows N] [--batch-size N] [--lookups N] [--versions-per-row N] [--temporal-rows N] [--output FILE]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if cfg.rows == 0
        || cfg.batch_size == 0
        || cfg.lookups == 0
        || cfg.versions_per_row < 2
        || cfg.temporal_rows == 0
    {
        return Err(
            "rows/batch/lookups/temporal-rows must be >0 and versions-per-row must be >=2".into(),
        );
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
    println!("Adaptive DB 1.6.2 Rust temporal-storage baseline benchmark");
    println!(
        "rows={} batch={} lookups={} versions/row={} temporal_rows={}",
        cfg.rows, cfg.batch_size, cfg.lookups, cfg.versions_per_row, cfg.temporal_rows
    );
    println!("NOTE: Database commit measurements include segmented WAL sync + Version/Current flush + checkpoint v2.\n");

    let dir = tempdir()?;
    let mut measurements = Vec::new();

    // B01: temporal VersionBTree insertion.
    let tree_dir = dir.path().join("version-btree");
    fs::create_dir_all(&tree_dir)?;
    let tree = VersionBTree::open(
        tree_dir.join("versions.idx"),
        tree_dir.join("versions.meta"),
        128,
    )?;
    let tree_entries = cfg.rows;
    measurements.push(measure("version_btree_insert", tree_entries, || {
        for i in 0..tree_entries {
            let key = VersionKey::new(
                RowId((i / cfg.versions_per_row) as u128),
                CommitTs((i % cfg.versions_per_row) as u64 + 1),
            );
            let loc = RowLocation {
                page_id: PageId((i / 8) as u64),
                slot_id: (i % 8) as u16,
            };
            tree.insert_at_lsn(key, loc, Lsn(i as u64 + 1)).unwrap();
        }
        tree.flush().unwrap();
    }));

    // B02: floor lookup in temporal B+Tree.
    let distinct_rows = (tree_entries / cfg.versions_per_row).max(1);
    measurements.push(measure("version_btree_get_floor", cfg.lookups, || {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..cfg.lookups {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let rid = (state as usize % distinct_rows) as u128;
            black_box(
                tree.get_floor(VersionKey::new(
                    RowId(rid),
                    CommitTs(cfg.versions_per_row as u64),
                ))
                .unwrap(),
            );
        }
    }));

    // B03: PersistentVersionStore materialization (heap + temporal B+Tree).
    let pvs_dir = dir.path().join("persistent-versions");
    let pvs = PersistentVersionStore::open(&pvs_dir)?;
    let pvs_entries = cfg.temporal_rows * (cfg.versions_per_row - 1);
    measurements.push(measure("persistent_version_put", pvs_entries, || {
        let mut lsn = 1u64;
        for rid in 0..cfg.temporal_rows {
            for v in 0..(cfg.versions_per_row - 1) {
                let hv = HistoricalVersion {
                    begin_ts: CommitTs(v as u64 + 1),
                    end_ts: CommitTs(v as u64 + 2),
                    value: Some(row((rid * 1000 + v) as i64)),
                };
                pvs.put_at_lsn(RowId(rid as u128), &hv, Lsn(lsn)).unwrap();
                lsn += 1;
            }
        }
        pvs.flush().unwrap();
    }));

    // B04: PersistentVersionStore temporal lookup.
    measurements.push(measure("persistent_version_get_at", cfg.lookups, || {
        for i in 0..cfg.lookups {
            let rid = (i % cfg.temporal_rows) as u128;
            let ts = CommitTs(((i % (cfg.versions_per_row - 1)) + 1) as u64);
            black_box(pvs.get_at(RowId(rid), ts).unwrap());
        }
    }));

    // B05: Database batched initial insert.
    let db_dir = dir.path().join("database");
    let db = Database::open(&db_dir)?;
    measurements.push(measure("durable_batched_insert", cfg.rows, || {
        let mut base = 0usize;
        while base < cfg.rows {
            let end = (base + cfg.batch_size).min(cfg.rows);
            let mut tx = db.begin();
            for i in base..end {
                tx.put(RowId(i as u128), row(i as i64));
            }
            black_box(db.commit(tx).unwrap());
            base = end;
        }
    }));

    // B06: Create historical versions through normal commits.
    let history_rows = cfg.temporal_rows.min(cfg.rows);
    let mut snapshots = Vec::new();
    measurements.push(measure(
        "durable_temporal_updates",
        history_rows * (cfg.versions_per_row - 1),
        || {
            for version in 1..cfg.versions_per_row {
                let mut tx = db.begin();
                for rid in 0..history_rows {
                    tx.put(RowId(rid as u128), row((version * 1_000_000 + rid) as i64));
                }
                let ts = db.commit(tx).unwrap();
                snapshots.push(ts);
            }
        },
    ));

    // B07: current lookup through persistent primary store.
    measurements.push(measure("database_current_get", cfg.lookups, || {
        for i in 0..cfg.lookups {
            black_box(db.get(RowId((i % cfg.rows) as u128)).unwrap());
        }
    }));

    // B08: temporal lookup through persistent VersionStore while DB is open.
    let first_snapshot = snapshots.first().copied().unwrap_or(CommitTs(1));
    measurements.push(measure("database_get_at_persistent", cfg.lookups, || {
        for i in 0..cfg.lookups {
            black_box(
                db.get_at(RowId((i % history_rows) as u128), first_snapshot)
                    .unwrap(),
            );
        }
    }));

    // B09: per-row history enumeration.
    let history_ops = cfg.lookups.min(20_000);
    measurements.push(measure("database_history", history_ops, || {
        for i in 0..history_ops {
            black_box(db.history(RowId((i % history_rows) as u128)).unwrap());
        }
    }));

    // B10: reopen and immediately query persistent history.
    drop(db);
    measurements.push(measure("database_reopen_temporal", 1, || {
        let reopened = Database::open(&db_dir).unwrap();
        black_box(reopened.get(RowId(0)).unwrap());
        black_box(reopened.get_at(RowId(0), first_snapshot).unwrap());
        black_box(reopened.history(RowId(0)).unwrap());
    }));

    // B11/B12: segmented WAL rotation, sync and complete sequential read.
    let wal_dir = dir.path().join("segmented-wal");
    let wal_records = cfg.rows.min(50_000);
    let mut writer = SegmentedWalWriter::open(&wal_dir, 16 * 1024)?;
    measurements.push(measure("segmented_wal_append_sync", wal_records, || {
        for i in 0..wal_records {
            writer
                .append(&WalRecord::Begin {
                    tx_id: TxId(i as u64 + 1),
                    snapshot_ts: CommitTs(i as u64),
                })
                .unwrap();
        }
        writer.sync().unwrap();
    }));
    drop(writer);
    measurements.push(measure("segmented_wal_read_all", wal_records, || {
        let records = SegmentedWalReader::read_all(&wal_dir).unwrap();
        assert_eq!(records.len(), wal_records);
        black_box(records);
    }));

    println!(
        "{:<31} {:>12} {:>14} {:>14} {:>14}",
        "benchmark", "operations", "elapsed_ms", "ops/s", "ns/op"
    );
    for m in &measurements {
        println!(
            "{:<31} {:>12} {:>14.3} {:>14.1} {:>14.1}",
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
        let mut json = String::from("{\n  \"milestone\": \"1.6.2\",\n  \"engine_base\": \"1.6.1-hardened\",\n  \"measurements\": [\n");
        for (i, m) in measurements.iter().enumerate() {
            let comma = if i + 1 == measurements.len() { "" } else { "," };
            json.push_str(&format!("    {{\"name\":\"{}\",\"operations\":{},\"elapsed_ns\":{},\"ops_per_sec\":{:.3},\"ns_per_op\":{:.3}}}{}\n", m.name,m.operations,m.elapsed.as_nanos(),m.ops_per_sec(),m.ns_per_op(),comma));
        }
        json.push_str("  ]\n}\n");
        fs::write(path, json)?;
        println!("\nJSON results written to {}", path.display());
    }
    Ok(())
}
