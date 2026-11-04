//! Tests of the schema-3 report builder.
use adb_bench::report::{legacy_timing, rate_per_second, validate, SCHEMA_VERSION};
use adb_bench::{Environment, MeasurementPlan, Report, SampleStats};
use serde_json::{json, Map, Value};

fn stats() -> SampleStats {
    SampleStats::from_batches(
        &[
            vec![1.0e6, 1.1e6, 0.9e6],
            vec![1.0e6, 1.05e6, 0.95e6],
            vec![1.0e6, 1.0e6, 1.0e6],
        ],
        4,
        false,
    )
}

fn report() -> Report {
    Report::new(
        "test",
        "2.5.4.1",
        &MeasurementPlan::quick(),
        &Environment::capture(),
    )
}

#[test]
fn a_finished_report_validates() {
    let mut r = report();
    r.add_case("scan", &stats(), Map::new());
    let doc = r.finish();
    assert_eq!(doc["schema"], SCHEMA_VERSION);
    assert_eq!(doc["milestone"], "2.5.4.1");
    assert!(validate(&doc).is_empty(), "{:?}", validate(&doc));
}

#[test]
fn legacy_keys_are_kept_next_to_the_timing_object() {
    let mut r = report();
    r.add_case("scan", &stats(), Map::new());
    let doc = r.finish();
    let case = &doc["cases"]["scan"];
    for key in [
        "mean_ms",
        "p50_ms",
        "p95_ms",
        "p99_ms",
        "min_ms",
        "max_ms",
        "stddev_ms",
        "round_medians_ms",
        "iterations",
    ] {
        assert!(case.get(key).is_some(), "legacy key {key} is missing");
    }
    assert_eq!(case["round_medians_ms"].as_array().unwrap().len(), 3);
    assert!(
        (case["p50_ms"].as_f64().unwrap()
            - case["timing"]["median_of_medians_ms"].as_f64().unwrap())
        .abs()
            < 1e-12
    );
    assert_eq!(case["timing"]["batches"], 3);
}

#[test]
fn extra_keys_win_over_the_legacy_ones() {
    let mut r = report();
    let mut extra = Map::new();
    extra.insert("rows_per_s".into(), json!(1234.5));
    extra.insert("mean_ms".into(), json!(-1.0));
    r.add_case("load", &stats(), extra);
    let doc = r.finish();
    assert_eq!(doc["cases"]["load"]["rows_per_s"], 1234.5);
    assert_eq!(doc["cases"]["load"]["mean_ms"], -1.0);
}

#[test]
fn the_validator_names_every_problem() {
    let problems = validate(&json!({"schema": 2, "cases": {"a": {"timing": {}}, "b": 5}}));
    assert!(problems.iter().any(|p| p.contains("`schema` must be 3")));
    assert!(problems
        .iter()
        .any(|p| p.contains("missing top-level key `plan`")));
    assert!(problems
        .iter()
        .any(|p| p.contains("$.cases.a.timing: timing lacks `stability`")));
    assert!(problems
        .iter()
        .any(|p| p.contains("case `b` is not an object")));
    assert_eq!(
        validate(&json!([1])),
        vec!["the document is not a JSON object".to_string()]
    );
    assert!(!validate(&json!({})).is_empty());
}

#[test]
fn plain_cases_need_a_reason_or_a_value() {
    let mut r = report();
    let mut values = Map::new();
    values.insert("value".into(), json!(42));
    r.add_plain_case("rows", values, None);
    r.add_plain_case("skipped", Map::new(), Some("no tmpfs for durable families"));
    r.add_plain_case("bad", Map::new(), None);
    let doc = r.finish();
    let problems = validate(&doc);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("case `bad`"));
    assert_eq!(
        doc["cases"]["skipped"]["skipped_reason"],
        "no tmpfs for durable families"
    );
}

#[test]
fn the_limit_decides_the_verdict_in_the_document() {
    let noisy =
        SampleStats::from_batches(&[vec![1.0e6; 3], vec![2.0e6; 3], vec![3.0e6; 3]], 1, false);
    let mut strict = report();
    strict.add_case("x", &noisy, Map::new());
    assert_eq!(
        strict.finish()["cases"]["x"]["timing"]["stability"],
        "unstable"
    );
    let mut lax = report().with_limit_percent(1000.0);
    lax.add_case("x", &noisy, Map::new());
    let doc = lax.finish();
    assert_eq!(doc["cases"]["x"]["timing"]["stability"], "stable");
    assert_eq!(
        doc["cases"]["x"]["timing"]["stability_limit_percent"],
        1000.0
    );
}

#[test]
fn rates_handle_a_zero_median() {
    let s = stats();
    assert!((rate_per_second(&s, 1000.0) - 1000.0 / 1e-3).abs() < 1e-6);
    let zero = SampleStats::from_batches(&[vec![0.0, 0.0]], 1, true);
    assert_eq!(rate_per_second(&zero, 10.0), 0.0);
    assert_eq!(legacy_timing(&zero)["p50_ms"], Value::from(0.0));
}

#[test]
fn timing_objects_are_found_anywhere_and_checked_for_consistency() {
    let mut r = report();
    r.add_case("ok", &stats(), Map::new());
    let mut doc = r.finish();
    // older reports keep their own layout: a timing object outside `cases`
    let mut bad = doc["cases"]["ok"]["timing"].clone();
    bad["stability"] = json!("great");
    bad["outlier_count"] = json!(3);
    bad["batches"] = json!(9);
    bad["median_of_medians_ms"] = json!(-1.0);
    bad["surprise"] = json!(true);
    doc["workloads"] = json!({"A": [{"timing": bad}]});
    let problems = validate(&doc);
    assert!(problems
        .iter()
        .any(|p| p.contains("$.workloads.A[0].timing: unknown timing key `surprise`")));
    assert!(problems.iter().any(|p| p.contains("stability `great`")));
    assert!(problems.iter().any(|p| p.contains("outlier_count 3")));
    assert!(problems.iter().any(|p| p.contains("batches 9")));
    assert!(problems
        .iter()
        .any(|p| p.contains("`median_of_medians_ms` must be")));
}
