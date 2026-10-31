//! Concurrency hardening of schema evolution: DDL, writes and scans running at the same time
//! keep every query consistent and every row valid, and the result survives a reopen (the
//! catalog and the rows are replayed from the log together).
use std::sync::atomic::{AtomicBool, Ordering};

use adb_core::{Row, RowId, Value, ValueType};
use adb_engine::{Database, DbError, EntitySpec, NewField, SchemaChange};
use adb_execution::{PhysicalPlan, ScanColumn, SlotId};
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

/// Every row of entity 1 as `(id, balance, newest extra column)` at the latest snapshot.
fn scan(db: &Database, extra: u32) -> Result<Vec<(Value, Value, Value)>, DbError> {
    let plan = PhysicalPlan::EntityScan {
        entity_id: 1,
        columns: [1, 2, extra]
            .iter()
            .enumerate()
            .map(|(slot, field_id)| ScanColumn {
                field_id: *field_id,
                slot: SlotId(slot as u32),
            })
            .collect(),
    };
    let mut cursor = db.execute(plan)?;
    let mut rows = Vec::new();
    while let Some(batch) = cursor.next_batch()? {
        for row in 0..batch.len() {
            rows.push((
                batch.value(row, SlotId(0)),
                batch.value(row, SlotId(1)),
                batch.value(row, SlotId(2)),
            ));
        }
    }
    Ok(rows)
}

/// One thread adds, renames and drops columns, one inserts and updates rows, one scans; no
/// operation fails unexpectedly, every scan sees well-formed rows, and a reopen reproduces the
/// final state exactly.
#[test]
fn ddl_writes_and_scans_run_concurrently() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(EntitySpec {
        name: "account".into(),
        fields: vec![
            field("id", ValueType::Int64, false, None),
            field("balance", ValueType::Int64, false, None),
        ],
        primary_key: "id".into(),
        entity_id: None,
    })
    .unwrap();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for i in 0..60 {
                let name = format!("c{i}");
                db.alter_entity(
                    1,
                    &SchemaChange::AddField {
                        field: field(&name, ValueType::Int64, false, Some(Value::Int64(i))),
                    },
                )
                .unwrap();
                db.alter_entity(
                    1,
                    &SchemaChange::RenameField {
                        from: name.clone(),
                        to: format!("{name}_r"),
                    },
                )
                .unwrap();
                if i % 3 != 0 {
                    db.alter_entity(
                        1,
                        &SchemaChange::DropField {
                            name: format!("{name}_r"),
                        },
                    )
                    .unwrap();
                }
            }
            done.store(true, Ordering::SeqCst);
        });
        scope.spawn(|| {
            let mut pk = 0i64;
            while !done.load(Ordering::SeqCst) || pk < 500 {
                pk += 1;
                let mut tx = db.begin();
                tx.put(
                    RowId::from_primary_key(1, pk),
                    Row::new()
                        .with_field(1, Value::Int64(pk))
                        .with_field(2, Value::Int64(pk * 10)),
                );
                db.commit(tx)
                    .expect("rows omitting added columns are completed");
                if pk % 7 == 0 {
                    let mut tx = db.begin();
                    tx.put(
                        RowId::from_primary_key(1, pk / 7),
                        Row::new()
                            .with_field(1, Value::Int64(pk / 7))
                            .with_field(2, Value::Int64(-1)),
                    );
                    db.commit(tx)
                        .expect("updates migrate rows to the latest version");
                }
            }
        });
        scope.spawn(|| {
            let mut last = 0;
            while !done.load(Ordering::SeqCst) {
                let rows = scan(&db, 3).expect("scans never fail during DDL");
                assert!(rows.len() >= last, "a later snapshot never loses rows");
                last = rows.len();
                for (id, balance, _) in &rows {
                    assert!(matches!(id, Value::Int64(_)), "{id:?}");
                    assert!(matches!(balance, Value::Int64(_)), "{balance:?}");
                }
            }
        });
    });
    let schema = db.schema(1).unwrap();
    assert_eq!(
        schema.version.0,
        1 + 60 * 2 + 40,
        "every DDL statement committed"
    );
    let kept: Vec<u32> = schema.active_fields().map(|f| f.id).collect();
    assert_eq!(kept.len(), 2 + 20, "every third added column is kept");
    let last = *kept.last().unwrap();
    let before = scan(&db, last).unwrap();
    assert!(
        before.iter().all(|row| matches!(row.2, Value::Int64(_))),
        "kept columns read their default or a value"
    );
    let epoch = db.schema_epoch();
    db.close().unwrap();
    drop(db);
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schema_epoch(), epoch);
    assert_eq!(
        scan(&db, last).unwrap(),
        before,
        "the reopened database reads the same rows"
    );
}
