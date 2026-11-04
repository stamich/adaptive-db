//! Tests of the benchmark context and of the setup-aware runner.
use std::time::{Duration, Instant};

use adb_bench::{measure, measure_with_setup, Context, MeasurementPlan};

#[test]
fn the_context_hands_out_directories_in_the_chosen_root() {
    let root = tempfile::tempdir().unwrap();
    let context = Context::new(Some(root.path().to_path_buf()));
    assert_eq!(context.data_root, root.path());
    let dir = context.tempdir().unwrap();
    assert!(dir.path().starts_with(root.path()));
    assert!(context
        .storage()
        .path
        .starts_with(root.path().canonicalize().unwrap().to_str().unwrap()));
    let probe = context.probe(dir.path()).expect("a writable directory");
    assert!(!probe.samples_ns.is_empty());
    assert!(context.probe(&dir.path().join("missing")).is_none());
}

#[test]
fn a_missing_root_is_an_error_not_a_panic() {
    let context = Context::new(Some("/definitely/not/a/directory".into()));
    assert!(context.tempdir().is_err());
}

/// A value whose destruction is slow, to prove the clock does not run while it is dropped.
struct SlowDrop;

/// Sleeps for 30 ms when dropped.
impl Drop for SlowDrop {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn dropping_the_previous_result_is_not_timed() {
    let plan = MeasurementPlan {
        warmup_invocations: 0,
        min_sample: Duration::ZERO,
        max_iterations: 1,
        batches: 1,
        samples_per_batch: 3,
    };
    let measured = measure(&plan, || SlowDrop);
    let slowest = measured.batches_ns[0].iter().cloned().fold(0.0, f64::max);
    assert!(
        slowest < 10e6,
        "a drop of 30 ms leaked into a sample: {slowest} ns"
    );
}

#[test]
fn setup_is_not_timed_and_the_state_reaches_the_run() {
    let plan = MeasurementPlan {
        warmup_invocations: 1,
        min_sample: Duration::ZERO,
        max_iterations: 1,
        batches: 2,
        samples_per_batch: 2,
    };
    let started = Instant::now();
    let measured = measure_with_setup(
        &plan,
        || {
            std::thread::sleep(Duration::from_millis(20));
            7u64
        },
        |state| state * 2,
    );
    assert_eq!(measured.last, 14);
    assert_eq!(measured.iterations_per_sample, 1);
    assert_eq!(measured.batches_ns.len(), 2);
    assert!(
        measured.batches_ns.iter().flatten().all(|ns| *ns < 5e6),
        "the 20 ms setup was timed"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "setup ran for warm-up and every sample"
    );
}
