//! The benchmark harness as the workloads binary uses it.
//!
//! The binary decides its data directory and its measurement plans once, at start, so the
//! sections ([`crate::consistency`], [`crate::schema`], the workloads themselves) need not pass a
//! context through every call. [`init`] stores the [`Context`]; the functions here read it.

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use adb_bench::report::case_json;
use adb_bench::stats::DEFAULT_STABLE_BATCH_MAD_PERCENT;
use adb_bench::{Context, MeasurementPlan, SampleStats};
use serde_json::{Map, Value as Json};
use tempfile::TempDir;

/// The context of this process; set once by [`init`].
static CONTEXT: OnceLock<Context> = OnceLock::new();

/// Stores the context (data directory `data_dir`, or `ADB_BENCH_DATA_DIR`, or the system
/// temporary directory) and returns it. A second call returns the first context.
pub fn init(data_dir: Option<PathBuf>) -> &'static Context {
    CONTEXT.get_or_init(|| Context::new(data_dir))
}

/// The context set by [`init`].
///
/// # Panics
///
/// Panics if [`init`] has not been called; `main` calls it first.
pub fn context() -> &'static Context {
    CONTEXT.get().expect("harness::init is called first")
}

/// A fresh temporary directory in the data directory. Same signature as
/// `tempfile::tempdir`, so the sections can use it by name.
pub fn tempdir() -> io::Result<TempDir> {
    context().tempdir()
}

/// The plan of CPU-bound cases (reads, plans, scans).
pub fn cpu_plan() -> MeasurementPlan {
    context().plan(false)
}

/// The plan of fsync-bound cases (loads, commits, recovery).
pub fn durable_plan() -> MeasurementPlan {
    context().plan(true)
}

/// The plan of durable cases whose single invocation takes a second or more (the dataset load,
/// thousands of commits): ten invocations instead of 27, so a disk with a slow `fsync` does not
/// turn one case into half an hour.
pub fn heavy_plan() -> MeasurementPlan {
    context().heavy_plan()
}

/// The JSON of one case: the legacy timing keys, the `timing` object and `extra`.
pub fn case(stats: &SampleStats, extra: Vec<(&str, Json)>) -> Json {
    let extra: Map<String, Json> = extra.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    case_json(stats, DEFAULT_STABLE_BATCH_MAD_PERCENT, extra)
}

/// Total invocations a measurement ran (samples times the iterations folded into each).
pub fn invocations(stats: &SampleStats, plan: &MeasurementPlan) -> u64 {
    plan.total_samples() as u64 * stats.iterations_per_sample
}
