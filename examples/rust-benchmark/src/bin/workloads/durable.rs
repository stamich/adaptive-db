//! The `durable-write` family: cases whose cost is dominated by `fsync`.
//!
//! These cases use the durable measurement plan (one invocation per sample, five batches of five)
//! and are reported next to an fsync probe taken before and after them, so a slow disk and a slow
//! engine can be told apart. The probe is a guide, not a correction: a commit may need more than
//! one `fsync`, so [`summary`] reports each commit's time as a multiple of the probe's median
//! rather than subtracting it.

use adb_bench::report::rate_per_second;
use adb_bench::{measure, FsyncProbe, SampleStats};
use adb_core::{Direction, RowId};
use adb_engine::{Database, IndexColumn, IndexSpec};
use serde_json::{json, Value as Json};

use crate::dataset;
use crate::harness::{case, durable_plan, tempdir};
use crate::{ix_account, Result};

/// Rows inserted per write-overhead measurement.
const WRITE_ROWS: u64 = 20_000;
/// Rows per commit in the write-overhead measurement.
const WRITE_BATCH: u64 = 100;

/// Inserts [`WRITE_ROWS`] `ix_account`-shaped rows (100 per commit) into fresh databases with
/// 0, 1 and 3 indexes; reports rows per second (with timing and spread) and the slowdown relative
/// to no index. Creating the database and the indexes is part of each invocation but is small next
/// to 200 commits.
pub fn write_overhead() -> Result<Json> {
    let entity = ix_account();
    let index = |name: &str, fields: &[u32], include: &[u32]| IndexSpec {
        name: name.into(),
        entity_id: entity,
        columns: fields
            .iter()
            .map(|field| IndexColumn {
                field_id: *field,
                direction: Direction::Asc,
            })
            .collect(),
        include: include.to_vec(),
    };
    let sets: [(&str, Vec<IndexSpec>); 3] = [
        ("0_indexes", vec![]),
        ("1_index", vec![index("w_balance", &[3], &[])]),
        (
            "3_indexes",
            vec![
                index("w_balance", &[3], &[]),
                index("w_owner", &[4], &[3]),
                index("w_region_balance", &[2, 3], &[]),
            ],
        ),
    ];
    let table = dataset::by_name("ix_account");
    let mut report = serde_json::Map::new();
    let mut baseline = None;
    for (name, specs) in sets {
        let measured = measure(&durable_plan(), || {
            let dir = tempdir().expect("data directory");
            let db = Database::open(dir.path()).expect("open");
            for spec in &specs {
                db.create_index(spec.clone()).expect("index");
            }
            let mut first = 1;
            while first <= WRITE_ROWS {
                let mut tx = db.begin();
                for id in first..first + WRITE_BATCH {
                    tx.put(
                        RowId::from_primary_key(entity, id as i64),
                        dataset::row(table, id, 1),
                    );
                }
                db.commit(tx).expect("commit");
                first += WRITE_BATCH;
            }
            (dir, db)
        });
        let stats = SampleStats::from_measured(&measured);
        let rate = rate_per_second(&stats, WRITE_ROWS as f64);
        let base = *baseline.get_or_insert(rate);
        report.insert(
            name.into(),
            case(
                &stats,
                vec![
                    ("rows", json!(WRITE_ROWS)),
                    ("rows_per_commit", json!(WRITE_BATCH)),
                    ("seconds", json!(stats.median_ns / 1e9)),
                    ("rows_per_s", json!(rate)),
                    ("slowdown_vs_no_index", json!(base / rate)),
                ],
            ),
        );
    }
    Ok(Json::Object(report))
}

/// A commit's time as a multiple of the probe's median `write` + `fdatasync`.
pub fn probe_multiple(commit_seconds: f64, probe: &FsyncProbe) -> Option<f64> {
    let probe_seconds = probe.p50_ns() / 1e9;
    (probe_seconds > 0.0 && commit_seconds > 0.0).then(|| commit_seconds / probe_seconds)
}

/// The `durable_write` section: the probe before and after, its drift, and for each durable case
/// the commit time, the batch spread and the commit time as a multiple of the probe. `overhead` is
/// the output of [`write_overhead`]; `consistency` that of the 2.5.0 section (workload I holds the
/// one-row and batched commit cases).
pub fn summary(
    before: &FsyncProbe,
    after: &FsyncProbe,
    overhead: &Json,
    consistency: &Json,
) -> Json {
    let mut cases = serde_json::Map::new();
    let mut add = |name: &str, rows_per_commit: f64, node: &Json| {
        let rows_per_s = node["rows_per_s"].as_f64().unwrap_or(0.0);
        if rows_per_s <= 0.0 || rows_per_commit <= 0.0 {
            return;
        }
        let commit_seconds = rows_per_commit / rows_per_s;
        cases.insert(
            name.into(),
            json!({
                "commits_per_s": 1.0 / commit_seconds,
                "commit_ms": commit_seconds * 1e3,
                "batch_mad_percent": node["timing"]["batch_mad_percent"],
                "stability": node["timing"]["stability"],
                "commit_as_probe_multiples": probe_multiple(commit_seconds, before),
            }),
        );
    };
    for name in ["0_indexes", "1_index", "3_indexes"] {
        add(
            &format!("writes_{name}"),
            WRITE_BATCH as f64,
            &overhead[name],
        );
    }
    let workloads = &consistency["workloads"]["I_commit_with_change_log"];
    add("one_row_per_commit", 1.0, &workloads["one_row_per_commit"]);
    add(
        "batched",
        workloads["batched"]["rows_per_commit"]
            .as_f64()
            .unwrap_or(0.0),
        &workloads["batched"],
    );
    json!({
        "fsync_probe_before": before.to_json(),
        "fsync_probe_after": after.to_json(),
        "probe_drift_percent": before.drift_percent(after),
        "cases": cases,
    })
}
