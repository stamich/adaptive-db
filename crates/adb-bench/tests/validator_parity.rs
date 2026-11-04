//! The Rust validator and `scripts/bench_validate.py` must agree on the same documents.
use std::path::PathBuf;
use std::process::Command;

use adb_bench::report::validate;
use adb_bench::{Environment, MeasurementPlan, Report, SampleStats};
use serde_json::{json, Map};

/// Path of the validator script in the repository.
fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/bench_validate.py")
}

/// Whether `python3` can be run (the test is skipped on machines without it).
fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Runs the script on a document and returns whether it accepted it.
fn script_accepts(doc: &serde_json::Value) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.json");
    std::fs::write(&path, serde_json::to_vec(doc).unwrap()).unwrap();
    Command::new("python3")
        .arg("-I")
        .arg(script())
        .arg(&path)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn both_validators_accept_a_built_report_and_reject_a_broken_one() {
    if !python_available() {
        eprintln!("python3 not found; parity test skipped");
        return;
    }
    let stats = SampleStats::from_batches(&[vec![1.0e6, 1.1e6], vec![1.0e6, 0.9e6]], 2, false);
    let mut report = Report::new(
        "parity",
        "2.5.4.1",
        &MeasurementPlan::quick(),
        &Environment::capture(),
    );
    report.add_case("scan", &stats, Map::new());
    let good = report.finish();
    assert!(validate(&good).is_empty());
    assert!(script_accepts(&good));

    let mut bad = good.clone();
    bad["cases"]["scan"]["timing"]["stability"] = json!("great");
    bad["cases"]["scan"]["timing"]["extra"] = json!(1);
    assert!(!validate(&bad).is_empty());
    assert!(!script_accepts(&bad));

    let mut missing = good;
    missing.as_object_mut().unwrap().remove("plan");
    assert!(!validate(&missing).is_empty());
    assert!(!script_accepts(&missing));
}
