//! Milestone 2.4.0 schema-evolution workloads, measured in Rust on a database of their own.
//!
//! The `sc_account` tables (`id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING NOT
//! NULL`) are loaded at schema version 1; then `ALTER TABLE ... ADD COLUMN currency STRING NOT
//! NULL DEFAULT 'EUR'` publishes version 2 without touching the rows. Rewriting a share of the
//! rows (lazy migration) gives three tables whose rows are 0 %, 50 % and 100 % at the latest
//! version, plus an unversioned control table that stores `currency` itself (the 2.3.0 way):
//!
//! * **F_virtual_default_point_read** — a point lookup of a row that predates the column (the
//!   default is filled in on read), next to the same lookup of a migrated row;
//! * **G_schema_adapted_scan** — a scan with a filter on the new column and an aggregate, over
//!   the three mixes and the control table (all four must return the same checksum);
//! * **H_stale_schema_epoch_reject** — how fast a plan of an older catalog epoch is refused,
//!   next to the accepted execution of the same plan;
//! * **alter_latency** — `ADD COLUMN` on a small and a large table: time and log bytes, which do
//!   not depend on the table's size because no row is rewritten;
//! * **write_validation** — insert throughput into a table with a schema (stamping, defaults,
//!   type checks) against one without.

use std::{path::Path, time::Instant};

use crate::harness::tempdir;
use adb_core::{Row, RowId, SchemaEpoch, Value, ValueType};
use adb_engine::{Database, EntitySpec, NewField, SchemaChange};
use adb_execution::{AggregateFunction, AggregateSpec, BinaryOp, Expr, PhysicalPlan, SlotId};
use serde_json::json;

use crate::harness::{case, cpu_plan, invocations};
use crate::{col, measure_latest, Result};
use adb_bench::SampleStats;

/// Rows of each `sc_account` table at scale factor 1.
const BASE_ROWS: u64 = 50_000;
/// Rows of the small table of the ALTER latency measurement.
const SMALL_ROWS: u64 = 1_000;
/// Rows per commit while loading.
const LOAD_BATCH: u64 = 1_000;
/// Rows inserted per write-validation measurement.
const WRITE_ROWS: u64 = 20_000;
/// Rows per commit in the write-validation measurement.
const WRITE_BATCH: u64 = 100;
/// Entity ids of the tables whose rows are 0 %, 50 % and 100 % at the latest version.
const MIXES: [(u64, &str, u64); 3] = [(30, "0", 0), (31, "50", 50), (32, "100", 100)];
/// Entity id of the unversioned control table.
const PLAIN: u64 = 33;
/// Entity id of the small table of the ALTER latency measurement.
const SMALL: u64 = 34;

/// A field declaration.
fn field(name: &str, value_type: ValueType) -> NewField {
    NewField {
        name: name.into(),
        value_type,
        nullable: false,
        default: None,
        references: None,
        id: None,
    }
}

/// `CREATE TABLE name (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING NOT NULL)`.
fn spec(name: &str, entity: u64) -> EntitySpec {
    EntitySpec {
        name: name.into(),
        fields: vec![
            field("id", ValueType::Int64),
            field("balance", ValueType::Int64),
            field("owner", ValueType::String),
        ],
        primary_key: "id".into(),
        entity_id: Some(entity),
    }
}

/// `ALTER TABLE ... ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR'`.
fn add_currency() -> SchemaChange {
    SchemaChange::AddField {
        field: NewField {
            default: Some(Value::String("EUR".into())),
            ..field("currency", ValueType::String)
        },
    }
}

/// Row `id` of an `sc_account` table; `currency` only for the unversioned control table.
fn row(id: u64, currency: bool) -> Row {
    let row = Row::new()
        .with_field(1, Value::Int64(id as i64))
        .with_field(2, Value::Int64(((id * 7_919) % 100_000) as i64))
        .with_field(3, Value::String(format!("owner-{}", id % 100)));
    if currency {
        row.with_field(4, Value::String("EUR".into()))
    } else {
        row
    }
}

/// Writes rows `ids` of `entity` (every `step`-th id), [`LOAD_BATCH`] per commit.
fn write(db: &Database, entity: u64, rows: u64, step: u64, currency: bool) -> Result<()> {
    let ids: Vec<u64> = (1..=rows).step_by(step as usize).collect();
    for chunk in ids.chunks(LOAD_BATCH as usize) {
        let mut tx = db.begin();
        for id in chunk {
            tx.put(
                RowId::from_primary_key(entity, *id as i64),
                row(*id, currency),
            );
        }
        db.commit(tx)?;
    }
    Ok(())
}

/// Bytes of the canonical log of the database in `dir`.
fn log_bytes(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir.join("wal"))? {
        total += entry?.metadata()?.len();
    }
    Ok(total)
}

/// Workload G: `SELECT COUNT(*), SUM(balance) FROM t WHERE currency = 'EUR'` (slots 0 =
/// balance, 1 = currency, 2 = count, 3 = sum).
fn adapted_scan(entity: u64) -> PhysicalPlan {
    PhysicalPlan::Aggregate {
        input: Box::new(PhysicalPlan::Filter {
            input: Box::new(PhysicalPlan::EntityScan {
                entity_id: entity,
                columns: vec![col(2, 0), col(4, 1)],
            }),
            predicate: Expr::Binary {
                left: Box::new(Expr::Slot { slot: SlotId(1) }),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Literal {
                    value: Value::String("EUR".into()),
                }),
            },
        }),
        group_by: vec![],
        aggregates: vec![
            AggregateSpec {
                function: AggregateFunction::Count,
                input: None,
                output: SlotId(2),
            },
            AggregateSpec {
                function: AggregateFunction::Sum,
                input: Some(SlotId(0)),
                output: SlotId(3),
            },
        ],
    }
}

/// Workload F: `SELECT id, balance, currency FROM t WHERE id = key`.
fn point_read(entity: u64, key: i64) -> PhysicalPlan {
    PhysicalPlan::PointLookup {
        row_id: RowId::from_primary_key(entity, key),
        columns: vec![col(1, 0), col(2, 1), col(4, 2)],
    }
}

/// The `currency` value a point lookup returns (checks that the default is filled in).
fn currency_of(db: &Database, plan: &PhysicalPlan) -> Result<String> {
    let mut cursor = db.execute(plan.clone())?;
    let batch = cursor
        .next_batch()?
        .ok_or("point lookup returned nothing")?;
    Ok(match batch.value(0, SlotId(2)) {
        Value::String(currency) => currency,
        other => format!("{other:?}"),
    })
}

/// Times stale and accepted `execute_planned` calls of a point lookup (workload H), with the
/// CPU-bound plan. A stale plan is refused before it reads anything.
fn stale_epoch(db: &Database, entity: u64) -> Result<serde_json::Value> {
    let plan = point_read(entity, 1);
    let current = db.schema_epoch();
    let stale = SchemaEpoch(current.0 - 1);
    let measurement_plan = cpu_plan();
    let time = |epoch: SchemaEpoch| -> serde_json::Value {
        let measured = adb_bench::measure(&measurement_plan, || {
            match db.execute_planned(plan.clone(), epoch, None) {
                Ok(mut cursor) => {
                    while cursor.next_batch().expect("batch").is_some() {}
                    0u64
                }
                Err(_) => 1,
            }
        });
        let stats = SampleStats::from_measured(&measured);
        let iterations = invocations(&stats, &measurement_plan);
        // Refusals are counted in a separate, untimed pass of as many calls as were timed, so
        // `refused == iterations` means every stale call was refused (and `0` that none was).
        let refused = (0..iterations)
            .filter(|_| db.execute_planned(plan.clone(), epoch, None).is_err())
            .count();
        case(
            &stats,
            vec![
                ("iterations", json!(iterations)),
                ("refused", json!(refused)),
            ],
        )
    };
    Ok(json!({
        "plan_epoch": stale.0,
        "catalog_epoch": current.0,
        "rejected": time(stale),
        "accepted_same_plan": time(current),
    }))
}

/// Inserts [`WRITE_ROWS`] rows into a fresh table with and without a schema.
fn write_validation() -> Result<serde_json::Value> {
    let mut report = serde_json::Map::new();
    let mut baseline = None;
    for (name, with_schema) in [("no_schema", false), ("schema", true)] {
        let dir = tempdir()?;
        let db = Database::open(dir.path())?;
        if with_schema {
            db.create_entity(spec("sc_write", 40))?;
            db.alter_entity(40, &add_currency())?;
        }
        let start = Instant::now();
        let mut first = 1;
        while first <= WRITE_ROWS {
            let mut tx = db.begin();
            for id in first..first + WRITE_BATCH {
                // The schema'd table fills in the default; the other stores what it is given.
                tx.put(
                    RowId::from_primary_key(40, id as i64),
                    row(id, !with_schema),
                );
            }
            db.commit(tx)?;
            first += WRITE_BATCH;
        }
        let seconds = start.elapsed().as_secs_f64();
        let rate = WRITE_ROWS as f64 / seconds;
        let base = *baseline.get_or_insert(rate);
        report.insert(
            name.into(),
            json!({"rows": WRITE_ROWS, "rows_per_commit": WRITE_BATCH, "seconds": seconds,
                   "rows_per_s": rate, "slowdown_vs_no_schema": base / rate}),
        );
        db.close()?;
    }
    Ok(serde_json::Value::Object(report))
}

/// Loads the `sc_account` tables at scale factor `scale` and measures every 2.4.0 workload.
pub fn report(scale: u64) -> Result<serde_json::Value> {
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let rows = BASE_ROWS * scale;
    let load = Instant::now();
    for (entity, mix, _) in MIXES {
        db.create_entity(spec(&format!("sc_account_{mix}"), entity))?;
        write(&db, entity, rows, 1, false)?;
    }
    write(&db, PLAIN, rows, 1, true)?;
    db.create_entity(spec("sc_small", SMALL))?;
    write(&db, SMALL, SMALL_ROWS, 1, false)?;
    let load_seconds = load.elapsed().as_secs_f64();

    // ALTER on the small and on a large table: time and log bytes (no row is rewritten).
    let mut alter = serde_json::Map::new();
    for (name, entity, table_rows) in [("small", SMALL, SMALL_ROWS), ("large", MIXES[0].0, rows)] {
        let before = log_bytes(dir.path())?;
        let start = Instant::now();
        db.alter_entity(entity, &add_currency())?;
        let ms = start.elapsed().as_secs_f64() * 1e3;
        alter.insert(
            name.into(),
            json!({"table_rows": table_rows, "ms": ms, "log_bytes": log_bytes(dir.path())? - before}),
        );
    }
    for (entity, _, _) in &MIXES[1..] {
        db.alter_entity(*entity, &add_currency())?;
    }
    // Lazy migration of a share of the rows: rewriting a row stamps it with version 2.
    for (entity, _, migrated) in MIXES {
        if let Some(step) = 100u64.checked_div(migrated) {
            write(&db, entity, rows, step, false)?;
        }
    }

    let mut scans = serde_json::Map::new();
    for (entity, mix, _) in MIXES {
        scans.insert(
            format!("{mix}_percent_migrated"),
            measure_latest(&db, &adapted_scan(entity), rows)?,
        );
    }
    scans.insert(
        "unversioned_control".into(),
        measure_latest(&db, &adapted_scan(PLAIN), rows)?,
    );
    let (old, migrated) = (point_read(MIXES[0].0, 1), point_read(MIXES[2].0, 1));
    let point = json!({
        "old_row": measure_latest(&db, &old, 1)?,
        "old_row_currency": currency_of(&db, &old)?,
        "migrated_row": measure_latest(&db, &migrated, 1)?,
        "migrated_row_currency": currency_of(&db, &migrated)?,
    });
    let stale = stale_epoch(&db, MIXES[0].0)?;
    let checks = json!({
        "schema_versions": MIXES.iter().map(|(entity, _, _)| db.schema(*entity).map(|s| s.version.0)).collect::<Vec<_>>(),
    });
    db.close()?;
    Ok(json!({
        "setup": {"rows_per_table": rows, "small_table_rows": SMALL_ROWS, "load_seconds": load_seconds},
        "workloads": {
            "F_virtual_default_point_read": point,
            "G_schema_adapted_scan": scans,
            "H_stale_schema_epoch_reject": stale,
        },
        "alter_latency": alter,
        "write_validation": write_validation()?,
        "checks": checks,
    }))
}
