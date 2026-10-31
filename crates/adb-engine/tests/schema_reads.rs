//! Schema-aware reads through plans: scans, point lookups and row fetches return rows adapted
//! to the schema; a query keeps the schemas of its start while DDL commits; AS OF reads
//! historical data through the latest schema.
use adb_core::{CommitTs, Direction, Row, RowId, SchemaVersion, Value, ValueType};
use adb_engine::{Database, EntitySpec, IndexColumn, IndexSpec, NewField, SchemaChange};
use adb_execution::{PhysicalPlan, ScanColumn, SlotId};
use adb_index::{IndexBounds, ScanDirection};
use tempfile::tempdir;

/// A field declaration.
fn field(name: &str, value_type: ValueType, nullable: bool, default: Option<Value>) -> NewField {
    NewField {
        name: name.into(),
        value_type,
        nullable,
        default,
        references: None,
        id: None,
    }
}

/// `account (id, balance, owner)` as entity 1 with `rows` rows written at version 1.
fn database(dir: &std::path::Path, rows: i64) -> Database {
    let db = Database::open(dir).unwrap();
    db.create_entity(EntitySpec {
        name: "account".into(),
        fields: vec![
            field("id", ValueType::Int64, false, None),
            field("balance", ValueType::Int64, true, None),
            field("owner", ValueType::String, true, None),
        ],
        primary_key: "id".into(),
        entity_id: None,
    })
    .unwrap();
    let mut tx = db.begin();
    for pk in 0..rows {
        tx.put(
            RowId::from_primary_key(1, pk),
            Row::new()
                .with_field(1, Value::Int64(pk))
                .with_field(2, Value::Int64(pk % 10))
                .with_field(3, Value::String(format!("o{pk}"))),
        );
    }
    db.commit(tx).unwrap();
    db
}

/// `ADD COLUMN status STRING NOT NULL DEFAULT 'ACTIVE'`.
fn add_status(db: &Database) {
    db.alter_entity(
        1,
        &SchemaChange::AddField {
            field: field(
                "status",
                ValueType::String,
                false,
                Some(Value::String("ACTIVE".into())),
            ),
        },
    )
    .unwrap();
}

/// Scan columns `fields` into slots 0...
fn columns(fields: &[u32]) -> Vec<ScanColumn> {
    fields
        .iter()
        .enumerate()
        .map(|(slot, field)| ScanColumn {
            field_id: *field,
            slot: SlotId(slot as u32),
        })
        .collect()
}

/// Every row of `plan` at `snapshot` (latest when `None`), slots 0..width.
fn run(
    db: &Database,
    plan: PhysicalPlan,
    snapshot: Option<CommitTs>,
    width: u32,
) -> Vec<Vec<Value>> {
    let mut cursor = match snapshot {
        Some(ts) => db.execute_at(plan, ts).unwrap(),
        None => db.execute(plan).unwrap(),
    };
    let mut out = Vec::new();
    while let Some(batch) = cursor.next_batch().unwrap() {
        for row in 0..batch.len() {
            out.push(
                (0..width)
                    .map(|slot| batch.value(row, SlotId(slot)))
                    .collect(),
            );
        }
    }
    out
}

/// Scans, point lookups and index row fetches all read old rows with the virtual default.
#[test]
fn every_access_path_reads_adapted_rows() {
    let dir = tempdir().unwrap();
    let db = database(dir.path(), 20);
    db.create_index(IndexSpec {
        name: "idx_balance".into(),
        entity_id: 1,
        columns: vec![IndexColumn {
            field_id: 2,
            direction: Direction::Asc,
        }],
        include: vec![],
    })
    .unwrap();
    add_status(&db);
    let active = Value::String("ACTIVE".into());

    let scanned = run(
        &db,
        PhysicalPlan::EntityScan {
            entity_id: 1,
            columns: columns(&[1, 4]),
        },
        None,
        2,
    );
    assert_eq!(scanned.len(), 20);
    assert!(scanned.iter().all(|row| row[1] == active));

    let looked_up = run(
        &db,
        PhysicalPlan::PointLookup {
            row_id: RowId::from_primary_key(1, 3),
            columns: columns(&[1, 4]),
        },
        None,
        2,
    );
    assert_eq!(looked_up, vec![vec![Value::Int64(3), active.clone()]]);

    let fetched = run(
        &db,
        PhysicalPlan::RowFetch {
            input: Box::new(PhysicalPlan::IndexScan {
                index_id: 0,
                bounds: IndexBounds {
                    prefix: vec![Value::Int64(4)],
                    ..IndexBounds::all()
                },
                direction: ScanDirection::Forward,
                columns: columns(&[2]),
                limit: None,
            }),
            columns: vec![ScanColumn {
                field_id: 4,
                slot: SlotId(1),
            }],
        },
        None,
        2,
    );
    assert_eq!(fetched.len(), 2);
    assert!(fetched.iter().all(|row| row[1] == active));
}

/// AS OF reads historical data through the latest schema: a snapshot before the ALTER shows
/// the default too, and a dropped column is hidden from it.
#[test]
fn historical_reads_use_the_latest_schema() {
    let dir = tempdir().unwrap();
    let db = database(dir.path(), 3);
    let before = db.latest_committed_ts();
    add_status(&db);
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "owner".into(),
        },
    )
    .unwrap();
    let rows = run(
        &db,
        PhysicalPlan::EntityScan {
            entity_id: 1,
            columns: columns(&[3, 4]),
        },
        Some(before),
        2,
    );
    assert!(rows
        .iter()
        .all(|row| row[0] == Value::Null && row[1] == Value::String("ACTIVE".into())));
    assert_eq!(
        db.get(RowId::from_primary_key(1, 0))
            .unwrap()
            .unwrap()
            .schema_version,
        SchemaVersion(3)
    );
}

/// A running query keeps the schemas of its start: a column dropped while it runs stays
/// readable to it; the next query no longer sees the column.
#[test]
fn running_queries_keep_their_schema() {
    let dir = tempdir().unwrap();
    let db = database(dir.path(), 3000);
    let plan = PhysicalPlan::EntityScan {
        entity_id: 1,
        columns: columns(&[3]),
    };
    let mut cursor = db.execute(plan.clone()).unwrap();
    let first = cursor.next_batch().unwrap().unwrap();
    assert!(first.len() < 3000, "the scan spans several pages");
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "owner".into(),
        },
    )
    .unwrap();
    let mut owners = first.len();
    while let Some(batch) = cursor.next_batch().unwrap() {
        for row in 0..batch.len() {
            assert_ne!(batch.value(row, SlotId(0)), Value::Null);
        }
        owners += batch.len();
    }
    assert_eq!(owners, 3000);
    let after = run(&db, plan, None, 1);
    assert!(after.iter().all(|row| row[0] == Value::Null));
}
