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

/// An index on a column added with a default holds the default for rows that predate it,
/// stays equal to its table through updates of those rows, and ANALYZE records the version.
#[test]
fn indexes_and_statistics_see_adapted_rows() {
    let dir = tempdir().unwrap();
    let db = database(dir.path(), 10);
    add_status(&db);
    db.create_index(IndexSpec {
        name: "idx_status".into(),
        entity_id: 1,
        columns: vec![IndexColumn {
            field_id: 4,
            direction: Direction::Asc,
        }],
        include: vec![],
    })
    .unwrap();
    let active = |db: &Database| {
        run(
            db,
            PhysicalPlan::IndexScan {
                index_id: 0,
                bounds: IndexBounds {
                    prefix: vec![Value::String("ACTIVE".into())],
                    ..IndexBounds::all()
                },
                direction: ScanDirection::Forward,
                columns: vec![],
                limit: None,
            },
            None,
            0,
        )
        .len()
    };
    assert_eq!(
        active(&db),
        10,
        "old rows are indexed under their virtual default"
    );
    // Updating an old row ends its entry (computed from the adapted before-image).
    let key = RowId::from_primary_key(1, 3);
    let mut tx = db.begin();
    let mut row = db.get_in_tx(&mut tx, key).unwrap().unwrap();
    row.fields.insert(4, Value::String("CLOSED".into()));
    tx.put(key, row);
    db.commit(tx).unwrap();
    assert_eq!(active(&db), 9);
    assert!(db.verify().unwrap().is_ok());
    drop(db);
    let db = Database::open(dir.path()).unwrap();
    assert!(db.verify().unwrap().is_ok());
    let statistics = db.analyze(1, &Default::default()).unwrap();
    assert_eq!(statistics.schema_version, 2);
    let status = statistics.columns.iter().find(|c| c.field_id == 4).unwrap();
    assert_eq!(status.null_count, 0, "ANALYZE reads the virtual default");
}

/// A plan made at an older catalog epoch is refused before it runs; a plan of the current
/// epoch runs.
#[test]
fn plans_of_another_epoch_are_refused() {
    let dir = tempdir().unwrap();
    let db = database(dir.path(), 3);
    let epoch = db.schema_epoch();
    let plan = PhysicalPlan::EntityScan {
        entity_id: 1,
        columns: columns(&[1]),
    };
    assert!(db.execute_planned(plan.clone(), epoch, None).is_ok());
    add_status(&db);
    match db.execute_planned(plan.clone(), epoch, None) {
        Err(adb_engine::DbError::Execution(adb_execution::ExecutionError::SchemaChanged {
            planned,
            current,
        })) => assert_eq!((planned, current), (epoch.0, epoch.0 + 1)),
        Err(other) => panic!("unexpected {other}"),
        Ok(_) => panic!("a stale plan ran"),
    }
    assert!(db.execute_planned(plan, db.schema_epoch(), None).is_ok());
}
