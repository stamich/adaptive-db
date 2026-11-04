//! The schema-3 report document.
//!
//! Reports of 2.5.0 to 2.5.4 held one mean (or one rate) per case and a `host` object with five
//! fields. Schema 3 keeps **every key those reports had** — so existing readers, charts and
//! `check-benchmarks.py` keep working — and adds what a comparison needs:
//!
//! * `schema` (`3`) and `milestone`, so a reader knows what it holds;
//! * `environment` (governor, frequency, toolchain) with `runtime_before` / `runtime_after`
//!   (frequency, temperature, load) and, for durable families, `storage` and `fsync_probe`;
//! * `plan`, the [`MeasurementPlan`] the cases were measured with;
//! * per case, a `timing` object: median of medians, batch MAD %, outliers, stability.
//!
//! `scripts/bench_validate.py` checks documents against this contract; the two must change together.

use serde_json::{json, Map, Value};

use crate::environment::{Environment, RuntimeSnapshot};
use crate::plan::MeasurementPlan;
use crate::probe::FsyncProbe;
use crate::stats::SampleStats;
use crate::storage::StorageInfo;

/// The report schema this crate writes.
pub const SCHEMA_VERSION: u64 = 3;

/// Keys every schema-3 document has at the top level.
pub const REQUIRED_TOP_LEVEL: [&str; 7] = [
    "schema",
    "milestone",
    "family",
    "environment",
    "runtime_before",
    "runtime_after",
    "plan",
];

/// Keys every `timing` object has.
pub const REQUIRED_TIMING: [&str; 10] = [
    "median_of_medians_ms",
    "batch_medians_ms",
    "batch_mad_percent",
    "outlier_count",
    "outlier_indices",
    "stability",
    "batches",
    "samples_per_batch",
    "iterations_per_sample",
    "quick",
];

/// A [`MeasurementPlan`] as a JSON object (durations in milliseconds).
pub fn plan_json(plan: &MeasurementPlan) -> Value {
    json!({
        "warmup_invocations": plan.warmup_invocations,
        "min_sample_ms": plan.min_sample.as_secs_f64() * 1000.0,
        "max_iterations": plan.max_iterations,
        "batches": plan.batches,
        "samples_per_batch": plan.samples_per_batch,
        "quick": plan.is_quick(),
    })
}

/// The `timing` object of one case. `limit_percent` is the batch-MAD limit of the verdict.
pub fn timing_json(stats: &SampleStats, limit_percent: f64) -> Value {
    json!({
        "median_of_medians_ms": stats.median_ns / 1e6,
        "batch_medians_ms": stats.batch_medians_ns.iter().map(|n| n / 1e6).collect::<Vec<_>>(),
        "batch_mad_percent": stats.batch_mad_percent,
        "outlier_count": stats.outlier_indices.len(),
        "outlier_indices": stats.outlier_indices,
        "stability": stats.stability(limit_percent).as_str(),
        "stability_limit_percent": limit_percent,
        "batches": stats.batch_medians_ns.len(),
        "samples_per_batch": stats.samples_per_batch,
        "iterations_per_sample": stats.iterations_per_sample,
        "quick": stats.quick,
    })
}

/// The keys of the pre-2.5.4.1 read-path reports, computed from the same samples. Times are in
/// milliseconds: `mean_ms`, `p50_ms`, `p95_ms`, `p99_ms`, `min_ms`, `max_ms`, `stddev_ms`,
/// `round_medians_ms` and `iterations`. `p50_ms` is the headline median of medians.
pub fn legacy_timing(stats: &SampleStats) -> Map<String, Value> {
    let ms = |ns: f64| ns / 1e6;
    let mut map = Map::new();
    map.insert("mean_ms".into(), json!(ms(stats.mean_ns)));
    map.insert("p50_ms".into(), json!(ms(stats.median_ns)));
    map.insert("p95_ms".into(), json!(ms(stats.p95_ns)));
    map.insert("p99_ms".into(), json!(ms(stats.p99_ns)));
    map.insert("min_ms".into(), json!(ms(stats.min_ns)));
    map.insert("max_ms".into(), json!(ms(stats.max_ns)));
    map.insert("stddev_ms".into(), json!(ms(stats.stddev_ns)));
    map.insert(
        "round_medians_ms".into(),
        json!(stats
            .batch_medians_ns
            .iter()
            .map(|n| ms(*n))
            .collect::<Vec<_>>()),
    );
    map.insert("iterations".into(), json!(stats.iterations_per_sample));
    map
}

/// Units per second when one invocation does `units` units of work (rows, operations, bytes).
/// Zero when the median is zero, so a degenerate sample never divides by zero.
pub fn rate_per_second(stats: &SampleStats, units: f64) -> f64 {
    if stats.median_ns > 0.0 {
        units / (stats.median_ns / 1e9)
    } else {
        0.0
    }
}

/// A report under construction. Build it with [`Report::new`], add cases and values, then call
/// [`Report::finish`].
#[derive(Debug, Clone)]
pub struct Report {
    /// The document: top-level keys and the `cases` map.
    doc: Map<String, Value>,
    /// Cases by name, in insertion order.
    cases: Map<String, Value>,
    /// Batch-MAD limit used for the verdict of cases added without their own limit.
    limit_percent: f64,
}

/// Construction and population of a [`Report`].
impl Report {
    /// Starts a document for `family` of `milestone`, measured with `plan` on `environment`.
    /// `runtime_before` is captured now; `runtime_after` is set by [`Report::finish`].
    pub fn new(
        family: &str,
        milestone: &str,
        plan: &MeasurementPlan,
        environment: &Environment,
    ) -> Self {
        let mut doc = Map::new();
        doc.insert("schema".into(), json!(SCHEMA_VERSION));
        doc.insert("milestone".into(), json!(milestone));
        doc.insert("family".into(), json!(family));
        doc.insert("environment".into(), environment.to_json());
        doc.insert(
            "runtime_before".into(),
            RuntimeSnapshot::capture().to_json(),
        );
        doc.insert("plan".into(), plan_json(plan));
        Self {
            doc,
            cases: Map::new(),
            limit_percent: crate::stats::DEFAULT_STABLE_BATCH_MAD_PERCENT,
        }
    }

    /// Sets the batch-MAD limit (percent) of the verdicts of cases added afterwards.
    pub fn with_limit_percent(mut self, limit_percent: f64) -> Self {
        self.limit_percent = limit_percent;
        self
    }

    /// Records the storage under the data directory (durable families).
    pub fn set_storage(&mut self, storage: &StorageInfo) {
        self.doc.insert("storage".into(), storage.to_json());
    }

    /// Records the fsync probe taken before and after the family.
    pub fn set_probe(&mut self, before: &FsyncProbe, after: &FsyncProbe) {
        self.doc.insert(
            "fsync_probe".into(),
            json!({
                "before": before.to_json(),
                "after": after.to_json(),
                "drift_percent": before.drift_percent(after),
            }),
        );
    }

    /// Adds or replaces a top-level key. Used for the keys a family has always written.
    pub fn set(&mut self, key: &str, value: Value) {
        self.doc.insert(key.into(), value);
    }

    /// Adds a case: the legacy timing keys, the `timing` object, and `extra` keys (rates,
    /// checksums, counts) written by the family. `extra` wins over the legacy keys.
    pub fn add_case(&mut self, name: &str, stats: &SampleStats, extra: Map<String, Value>) {
        let mut case = legacy_timing(stats);
        case.insert("timing".into(), timing_json(stats, self.limit_percent));
        for (key, value) in extra {
            case.insert(key, value);
        }
        self.cases.insert(name.into(), Value::Object(case));
    }

    /// Adds a case that has no timing samples (a count, a checksum, a skipped case). `reason`
    /// is recorded as `skipped_reason` when given.
    pub fn add_plain_case(&mut self, name: &str, values: Map<String, Value>, reason: Option<&str>) {
        let mut case = values;
        if let Some(reason) = reason {
            case.insert("skipped_reason".into(), json!(reason));
        }
        self.cases.insert(name.into(), Value::Object(case));
    }

    /// Completes the document: captures `runtime_after` and inserts `cases`.
    pub fn finish(mut self) -> Value {
        self.doc
            .insert("runtime_after".into(), RuntimeSnapshot::capture().to_json());
        self.doc.insert("cases".into(), Value::Object(self.cases));
        Value::Object(self.doc)
    }
}

/// Checks a schema-3 document against the contract and returns every problem found.
///
/// An empty vector means the document is valid. The checks mirror `scripts/bench_validate.py`:
/// required keys, the schema number, and the shape of every case's `timing`.
pub fn validate(doc: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(top) = doc.as_object() else {
        return vec!["the document is not a JSON object".into()];
    };
    for key in REQUIRED_TOP_LEVEL {
        if !top.contains_key(key) {
            problems.push(format!("missing top-level key `{key}`"));
        }
    }
    if top.get("schema").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        problems.push(format!("`schema` must be {SCHEMA_VERSION}"));
    }
    match top.get("cases").and_then(Value::as_object) {
        None => problems.push("missing `cases` object".into()),
        Some(cases) => {
            for (name, case) in cases {
                let Some(case) = case.as_object() else {
                    problems.push(format!("case `{name}` is not an object"));
                    continue;
                };
                if let Some(timing) = case.get("timing") {
                    for key in REQUIRED_TIMING {
                        if timing.get(key).is_none() {
                            problems.push(format!("case `{name}`: timing lacks `{key}`"));
                        }
                    }
                } else if !case.contains_key("skipped_reason") && !case.contains_key("value") {
                    problems.push(format!(
                        "case `{name}` has neither `timing` nor `skipped_reason`"
                    ));
                }
            }
        }
    }
    problems
}
