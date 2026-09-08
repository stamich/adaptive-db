//! Zero-extra-dependency benchmark harness for Adaptive DB Milestone 1.0.2.
//!
//! The harness is intentionally implemented only with the Rust standard
//! library so that the early milestone does not acquire a benchmarking
//! framework dependency. Run it with `--release` for meaningful comparisons.
//!
//! It measures the real Milestone 1.0.1/1.0.2 execution paths:
//! - durable commits (including WAL sync),
//! - batched durable writes,
//! - current point reads,
//! - historical MVCC reads,
//! - WAL recovery.

use std::{
    env,
    fs::{self, File},
    hint::black_box,
    io::Write,
    path::{Path, PathBuf},
    process,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use adb_core::{CommitTs, Row, RowId, Value};
use adb_engine::Database;

#[derive(Debug, Clone)]
struct Config {
    rows: usize,
    batch_size: usize,
    lookups: usize,
    warmup_lookups: usize,
    output: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rows: 10_000,
            batch_size: 100,
            lookups: 100_000,
            warmup_lookups: 10_000,
            output: None,
        }
    }
}

#[derive(Debug)]
struct Measurement {
    name: &'static str,
    operations: usize,
    elapsed: Duration,
    samples_ns: Vec<u64>,
}

impl Measurement {
    fn throughput(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.operations as f64 / self.elapsed.as_secs_f64()
    }

    fn p50_ns(&self) -> Option<u64> {
        percentile(&self.samples_ns, 50)
    }

    fn p95_ns(&self) -> Option<u64> {
        percentile(&self.samples_ns, 95)
    }

    fn p99_ns(&self) -> Option<u64> {
        percentile(&self.samples_ns, 99)
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("benchmark failed: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let root = benchmark_path();
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root)?;

    println!("Adaptive DB — Milestone 1.0.2 Rust benchmark baseline");
    println!("mode: release build strongly recommended");
    println!("rows={}, batch_size={}, lookups={}, warmup_lookups={}",
        config.rows, config.batch_size, config.lookups, config.warmup_lookups);
    println!();

    // Keep fsync-per-commit work bounded because this is intentionally a very
    // expensive durability path. Up to 2,000 single-row commits is enough to
    // establish the early baseline while the batched workload scales to rows.
    let single_rows = config.rows.min(2_000);
    let single_path = root.join("single");
    let single = benchmark_durable_single(&single_path, single_rows)?;
    print_measurement(&single);

    let batch_path = root.join("batched");
    let (db, batch, first_commit) = benchmark_durable_batched(
        &batch_path,
        config.rows,
        config.batch_size.max(1),
    )?;
    print_measurement(&batch);

    warmup_reads(&db, config.rows, config.warmup_lookups);

    let gets = benchmark_get(&db, config.rows, config.lookups);
    print_measurement(&gets);

    let historical = benchmark_get_at(&db, config.rows, config.lookups, first_commit);
    print_measurement(&historical);

    drop(db);
    let recovery = benchmark_recovery(&batch_path, config.rows)?;
    print_measurement(&recovery);

    let measurements = [&single, &batch, &gets, &historical, &recovery];
    if let Some(output) = &config.output {
        write_json(output, &config, &measurements)?;
        println!("results: {}", output.display());
    }

    println!();
    println!("Methodology notes:");
    println!("- durable write timings include WAL append + WAL sync/fdatasync path used by Database::commit");
    println!("- point-read percentile timings include Instant measurement overhead; use throughput as the primary baseline");
    println!("- the database current/version stores are in-memory in this milestone; recovery rebuilds them from the WAL");
    println!("- compare releases on the same machine, filesystem, build profile and arguments");

    let _ = fs::remove_dir_all(root);
    Ok(())
}

fn benchmark_durable_single(path: &Path, rows: usize) -> Result<Measurement, Box<dyn std::error::Error>> {
    let db = Database::open(path)?;
    let mut samples = Vec::with_capacity(rows);
    let total_start = Instant::now();

    for i in 0..rows {
        let started = Instant::now();
        let mut tx = db.begin();
        tx.put(RowId(i as u128), benchmark_row(i));
        db.commit(tx)?;
        samples.push(duration_ns(started.elapsed()));
    }

    Ok(Measurement {
        name: "durable_single_row_commit",
        operations: rows,
        elapsed: total_start.elapsed(),
        samples_ns: samples,
    })
}

fn benchmark_durable_batched(
    path: &Path,
    rows: usize,
    batch_size: usize,
) -> Result<(Database, Measurement, CommitTs), Box<dyn std::error::Error>> {
    let db = Database::open(path)?;
    let mut samples = Vec::with_capacity(rows.div_ceil(batch_size));
    let total_start = Instant::now();
    let mut first_commit = CommitTs(0);

    for start in (0..rows).step_by(batch_size) {
        let end = (start + batch_size).min(rows);
        let started = Instant::now();
        let mut tx = db.begin();
        for i in start..end {
            tx.put(RowId(i as u128), benchmark_row(i));
        }
        let commit_ts = db.commit(tx)?;
        if first_commit.0 == 0 {
            first_commit = commit_ts;
        }
        samples.push(duration_ns(started.elapsed()));
    }

    Ok((
        db,
        Measurement {
            name: "durable_batched_insert_rows",
            operations: rows,
            elapsed: total_start.elapsed(),
            samples_ns: samples,
        },
        first_commit,
    ))
}

fn warmup_reads(db: &Database, rows: usize, lookups: usize) {
    if rows == 0 {
        return;
    }
    let mut state = 0x9e3779b97f4a7c15_u64;
    for _ in 0..lookups {
        state = xorshift64(state);
        let row_id = RowId((state as usize % rows) as u128);
        black_box(db.get(row_id));
    }
}

fn benchmark_get(db: &Database, rows: usize, lookups: usize) -> Measurement {
    let mut samples = Vec::with_capacity(lookups);
    let mut state = 0xd1b54a32d192ed03_u64;
    let total_start = Instant::now();

    if rows > 0 {
        for _ in 0..lookups {
            state = xorshift64(state);
            let row_id = RowId((state as usize % rows) as u128);
            let started = Instant::now();
            black_box(db.get(row_id));
            samples.push(duration_ns(started.elapsed()));
        }
    }

    Measurement {
        name: "current_point_get",
        operations: if rows == 0 { 0 } else { lookups },
        elapsed: total_start.elapsed(),
        samples_ns: samples,
    }
}

fn benchmark_get_at(db: &Database, rows: usize, lookups: usize, ts: CommitTs) -> Measurement {
    // The first batch is guaranteed to exist at first_commit. Restrict IDs to
    // that initial range by simply probing row 0; the benchmark measures the
    // historical version lookup path rather than dataset coverage.
    let total_start = Instant::now();
    let mut samples = Vec::with_capacity(lookups);

    if rows > 0 && ts.0 > 0 {
        for _ in 0..lookups {
            let started = Instant::now();
            black_box(db.get_at(RowId(0), ts));
            samples.push(duration_ns(started.elapsed()));
        }
    }

    Measurement {
        name: "historical_get_at",
        operations: if rows == 0 || ts.0 == 0 { 0 } else { lookups },
        elapsed: total_start.elapsed(),
        samples_ns: samples,
    }
}

fn benchmark_recovery(path: &Path, rows: usize) -> Result<Measurement, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let db = Database::open(path)?;
    let elapsed = started.elapsed();

    if rows > 0 {
        let last = RowId((rows - 1) as u128);
        if db.get(last).is_none() {
            return Err("recovery verification failed: last committed row is missing".into());
        }
    }

    Ok(Measurement {
        name: "wal_recovery_open",
        operations: rows,
        elapsed,
        samples_ns: Vec::new(),
    })
}

fn benchmark_row(i: usize) -> Row {
    Row::new()
        .with_field(1, Value::Int64(i as i64))
        .with_field(2, Value::Bool(i % 2 == 0))
        .with_field(3, Value::String(format!("row-{i:016}")))
}

fn print_measurement(m: &Measurement) {
    println!("{}", m.name);
    println!("  operations : {}", m.operations);
    println!("  elapsed    : {:.6} s", m.elapsed.as_secs_f64());
    println!("  throughput : {:.2} ops/s", m.throughput());
    if let (Some(p50), Some(p95), Some(p99)) = (m.p50_ns(), m.p95_ns(), m.p99_ns()) {
        println!("  latency    : p50={} ns, p95={} ns, p99={} ns", p50, p95, p99);
    }
    println!();
}

fn percentile(samples: &[u64], pct: usize) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) * pct) / 100;
    sorted.get(index).copied()
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

fn xorshift64(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rows" => config.rows = parse_usize(&arg, args.next())?,
            "--batch-size" => config.batch_size = parse_usize(&arg, args.next())?,
            "--lookups" => config.lookups = parse_usize(&arg, args.next())?,
            "--warmup-lookups" => config.warmup_lookups = parse_usize(&arg, args.next())?,
            "--output" => {
                let value = args.next().ok_or("--output requires a path")?;
                config.output = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                print_help();
                process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(config)
}

fn parse_usize(flag: &str, value: Option<String>) -> Result<usize, Box<dyn std::error::Error>> {
    let raw = value.ok_or_else(|| format!("{flag} requires a value"))?;
    Ok(raw.parse::<usize>().map_err(|_| format!("invalid value for {flag}: {raw}"))?)
}

fn print_help() {
    println!("adb-benchmark [OPTIONS]");
    println!("  --rows N             rows loaded into the benchmark database (default: 10000)");
    println!("  --batch-size N       rows per durable batch commit (default: 100)");
    println!("  --lookups N          measured current/historical reads (default: 100000)");
    println!("  --warmup-lookups N   unmeasured read warmup (default: 10000)");
    println!("  --output PATH        write machine-readable JSON results");
}

fn write_json(
    path: &Path,
    config: &Config,
    measurements: &[&Measurement],
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut file = File::create(path)?;
    writeln!(file, "{{")?;
    writeln!(file, "  \"milestone\": \"1.0.2\",")?;
    writeln!(file, "  \"engine_scope\": \"milestone-1.0.1-hardened\",")?;
    writeln!(file, "  \"config\": {{\"rows\": {}, \"batch_size\": {}, \"lookups\": {}, \"warmup_lookups\": {}}},",
        config.rows, config.batch_size, config.lookups, config.warmup_lookups)?;
    writeln!(file, "  \"measurements\": [")?;

    for (index, m) in measurements.iter().enumerate() {
        let comma = if index + 1 == measurements.len() { "" } else { "," };
        writeln!(
            file,
            "    {{\"name\": \"{}\", \"operations\": {}, \"elapsed_seconds\": {:.9}, \"throughput_ops_s\": {:.3}, \"p50_ns\": {}, \"p95_ns\": {}, \"p99_ns\": {}}}{}",
            m.name,
            m.operations,
            m.elapsed.as_secs_f64(),
            m.throughput(),
            json_u64(m.p50_ns()),
            json_u64(m.p95_ns()),
            json_u64(m.p99_ns()),
            comma,
        )?;
    }

    writeln!(file, "  ]")?;
    writeln!(file, "}}")?;
    Ok(())
}

fn json_u64(value: Option<u64>) -> String {
    value.map(|number| number.to_string()).unwrap_or_else(|| "null".to_owned())
}

fn benchmark_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    env::temp_dir().join(format!("adaptive-db-benchmark-{}-{nonce}", process::id()))
}
