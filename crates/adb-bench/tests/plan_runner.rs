//! Tests of the measurement plan and the runner.
use std::time::Duration;

use adb_bench::{measure, MeasurementPlan};

#[test]
fn release_plan_has_the_documented_shape() {
    let plan = MeasurementPlan::release();
    assert_eq!((plan.batches, plan.samples_per_batch), (3, 7));
    assert_eq!(plan.min_sample, Duration::from_millis(50));
    assert_eq!(plan.total_samples(), 21);
    assert!(!plan.is_quick());
}

#[test]
fn quick_plans_are_marked_quick() {
    assert!(MeasurementPlan::quick().is_quick());
    assert!(MeasurementPlan::durable_quick().is_quick());
    assert!(!MeasurementPlan::durable().is_quick());
}

#[test]
fn environment_selects_the_plan() {
    let none = |_: &str| None;
    assert_eq!(
        MeasurementPlan::from_lookup(false, none),
        MeasurementPlan::release()
    );
    assert_eq!(
        MeasurementPlan::from_lookup(true, none),
        MeasurementPlan::durable()
    );
    let quick = |name: &str| (name == "ADB_BENCH_QUICK").then(|| "1".to_string());
    assert_eq!(
        MeasurementPlan::from_lookup(false, quick),
        MeasurementPlan::quick()
    );
    assert_eq!(
        MeasurementPlan::from_lookup(true, quick),
        MeasurementPlan::durable_quick()
    );
    // Anything but "1" is not quick.
    let not_quick = |name: &str| (name == "ADB_BENCH_QUICK").then(|| "0".to_string());
    assert_eq!(
        MeasurementPlan::from_lookup(false, not_quick),
        MeasurementPlan::release()
    );
}

#[test]
fn minimum_sample_time_can_be_overridden_for_cpu_bound_plans_only() {
    let lookup = |name: &str| (name == "ADB_BENCH_MIN_SAMPLE_MS").then(|| "7".to_string());
    assert_eq!(
        MeasurementPlan::from_lookup(false, lookup).min_sample,
        Duration::from_millis(7)
    );
    assert_eq!(
        MeasurementPlan::from_lookup(true, lookup).min_sample,
        Duration::ZERO
    );
    let garbage = |name: &str| (name == "ADB_BENCH_MIN_SAMPLE_MS").then(|| "x".to_string());
    assert_eq!(
        MeasurementPlan::from_lookup(false, garbage),
        MeasurementPlan::release()
    );
}

#[test]
fn a_durable_plan_takes_one_invocation_per_sample() {
    let plan = MeasurementPlan::durable();
    let mut calls = 0u64;
    let measured = measure(&plan, || {
        calls += 1;
        calls
    });
    // one call to start, the warm-up, one calibration group of one call, then the samples.
    let expected = 1 + u64::from(plan.warmup_invocations) + 1 + u64::from(plan.total_samples());
    assert_eq!(calls, expected);
    assert_eq!(measured.last, expected);
    assert_eq!(measured.iterations_per_sample, 1);
    assert_eq!(measured.batches_ns.len(), plan.batches as usize);
    assert!(measured
        .batches_ns
        .iter()
        .all(|b| b.len() == plan.samples_per_batch as usize));
}

#[test]
fn a_fast_closure_is_folded_into_many_iterations() {
    let plan = MeasurementPlan::quick();
    let mut x = 0u64;
    let measured = measure(&plan, || {
        x = x.wrapping_add(1);
        x
    });
    assert!(
        measured.iterations_per_sample > 1,
        "a nanosecond closure needs folding"
    );
    assert!(measured
        .batches_ns
        .iter()
        .flatten()
        .all(|ns| *ns >= 0.0 && ns.is_finite()));
}

#[test]
fn iterations_never_exceed_the_cap() {
    let plan = MeasurementPlan {
        min_sample: Duration::from_secs(3600),
        max_iterations: 8,
        ..MeasurementPlan::quick()
    };
    let measured = measure(&plan, || 1u8);
    assert_eq!(measured.iterations_per_sample, 8);
}

#[test]
fn the_heavy_plan_takes_ten_invocations_and_turns_quick_on_request() {
    let heavy = MeasurementPlan::heavy();
    assert_eq!(heavy.warmup_invocations + heavy.total_samples(), 10);
    assert!(!heavy.is_quick());
    assert_eq!(heavy.max_iterations, 1);
    assert_eq!(MeasurementPlan::heavy_from_lookup(|_| None), heavy);
    let quick = MeasurementPlan::heavy_from_lookup(|name| {
        (name == "ADB_BENCH_QUICK").then(|| "1".to_string())
    });
    assert_eq!(quick, MeasurementPlan::durable_quick());
}
