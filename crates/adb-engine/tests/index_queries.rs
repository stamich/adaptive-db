//! Index scans and row fetches executed by the engine: snapshot visibility, the range of
//! snapshots an index serves, and plans the index cannot answer.
mod common;

use adb_core::{CommitTs, Direction, Row, RowId, Value};
use adb_engine::{Database, IndexColumn, IndexSpec};
use adb_execution::{PhysicalPlan, ScanColumn, SlotId};
use adb_index::{IndexBounds, ScanDirection, ValueBound};
use tempfile::tempdir;

/// Entity of the accounts.
const ACCOUNT: u64 = 3;

/// Writes account `pk` with `balance` and owner `o<pk>` in its own commit.
fn set(db: &Database, pk: i64, balance: i64) -> CommitTs {
    let mut tx = db.begin();
    tx.put(
        RowId::from_primary_key(ACCOUNT, pk),
        Row::new()
            .with_field(1, Value::Int64(pk))
            .with_field(2, Value::Int64(balance))
            .with_field(3, Value::String(format!("o{pk}"))),
    );
    db.commit(tx).unwrap()
}

/// A database with accounts -50..50 (balance pk.rem_euclid(10)) and idx_balance INCLUDE owner.
fn database(dir: &std::path::Path) -> Database {
    let db = Database::open(dir).unwrap();
    for pk in -50..50 {
        set(&db, pk, pk.rem_euclid(10));
    }
    db.create_index(IndexSpec {
        name: "idx_balance".into(),
        entity_id: ACCOUNT,
        columns: vec![IndexColumn {
            field_id: 2,
            direction: Direction::Asc,
        }],
        include: vec![3],
    })
    .unwrap();
    db
}

/// `IndexScan` of idx_balance (id 0) reading `fields` into slots 0.., with `bounds`.
fn scan(bounds: IndexBounds, direction: ScanDirection, fields: &[u32]) -> PhysicalPlan {
    PhysicalPlan::IndexScan {
        index_id: 0,
        bounds,
        direction,
        columns: fields
            .iter()
            .enumerate()
            .map(|(slot, field)| ScanColumn {
                field_id: *field,
                slot: SlotId(slot as u32),
            })
            .collect(),
        limit: None,
    }
}

/// Equality on balance.
fn equal(balance: i64) -> IndexBounds {
    IndexBounds {
        prefix: vec![Value::Int64(balance)],
        ..IndexBounds::all()
    }
}

/// Runs `plan` at `snapshot` (latest if `None`) and returns the values of `slots` per row.
fn run(
    db: &Database,
    plan: PhysicalPlan,
    snapshot: Option<CommitTs>,
    slots: &[u32],
) -> Result<Vec<Vec<Value>>, String> {
    let mut cursor = match snapshot {
        Some(ts) => db.execute_at(plan, ts),
        None => db.execute(plan),
    }
    .map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    while let Some(batch) = cursor.next_batch().map_err(|error| error.to_string())? {
        for row in 0..batch.len() {
            rows.push(
                slots
                    .iter()
                    .map(|slot| batch.value(row, SlotId(*slot)))
                    .collect(),
            );
        }
    }
    Ok(rows)
}

/// A covering scan returns key and covered columns in index order (ties by primary key).
#[test]
fn covering_scans_read_only_the_index() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    let rows = run(
        &db,
        scan(equal(3), ScanDirection::Forward, &[2, 3]),
        None,
        &[0, 1],
    )
    .unwrap();
    let owners: Vec<Value> = rows.iter().map(|row| row[1].clone()).collect();
    let expected: Vec<Value> = [-47, -37, -27, -17, -7, 3, 13, 23, 33, 43]
        .iter()
        .map(|pk| Value::String(format!("o{pk}")))
        .collect();
    assert_eq!(owners, expected);
    let range = IndexBounds {
        prefix: vec![],
        lower: Some(ValueBound {
            value: Value::Int64(8),
            inclusive: true,
        }),
        upper: None,
    };
    let backward = run(&db, scan(range, ScanDirection::Backward, &[2]), None, &[0]).unwrap();
    assert_eq!(backward.len(), 20);
    assert_eq!(backward[0], vec![Value::Int64(9)]);
    assert_eq!(backward[19], vec![Value::Int64(8)]);
}

/// A row fetch adds the columns the index does not hold; reading such a column from the index
/// itself is refused.
#[test]
fn row_fetch_reads_other_columns() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    let plan = PhysicalPlan::RowFetch {
        input: Box::new(scan(equal(0), ScanDirection::Forward, &[])),
        columns: vec![ScanColumn {
            field_id: 1,
            slot: SlotId(5),
        }],
    };
    let ids: Vec<Value> = run(&db, plan, None, &[5])
        .unwrap()
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    assert_eq!(
        ids,
        [-50, -40, -30, -20, -10, 0, 10, 20, 30, 40]
            .map(Value::Int64)
            .to_vec()
    );
    let error = run(
        &db,
        scan(equal(0), ScanDirection::Forward, &[1]),
        None,
        &[0],
    )
    .unwrap_err();
    assert!(error.contains("neither key nor covered"), "{error}");
}

/// Scans see the snapshot: an older snapshot after the index was built sees older entries.
#[test]
fn scans_are_snapshot_consistent() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    let before = db.latest_committed_ts();
    set(&db, 3, 100); // moves account 3 from balance 3 to 100
    let old = run(
        &db,
        scan(equal(3), ScanDirection::Forward, &[3]),
        Some(before),
        &[0],
    )
    .unwrap();
    let new = run(
        &db,
        scan(equal(3), ScanDirection::Forward, &[3]),
        None,
        &[0],
    )
    .unwrap();
    assert_eq!(old.len(), 10);
    assert_eq!(new.len(), 9);
    let moved = run(
        &db,
        scan(equal(100), ScanDirection::Forward, &[3]),
        None,
        &[0],
    )
    .unwrap();
    assert_eq!(moved, vec![vec![Value::String("o3".into())]]);
}

/// An index cannot answer snapshots before it was built, history it vacuumed, or anything
/// after it was dropped.
#[test]
fn unusable_snapshots_and_dropped_indexes_fail() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let early = set(&db, 1, 1);
    set(&db, 7, 7);
    drop(db);
    let db = {
        let db = Database::open(dir.path()).unwrap();
        db.create_index(IndexSpec {
            name: "idx_balance".into(),
            entity_id: ACCOUNT,
            columns: vec![IndexColumn {
                field_id: 2,
                direction: Direction::Asc,
            }],
            include: vec![3],
        })
        .unwrap();
        db
    };
    // The snapshot before the only commit predates the index.
    let error = run(
        &db,
        scan(equal(1), ScanDirection::Forward, &[2]),
        Some(CommitTs(early.0 - 1)),
        &[0],
    )
    .unwrap_err();
    assert!(error.contains("cannot serve snapshot"), "{error}");

    let built = db.latest_committed_ts();
    set(&db, 1, 2);
    db.vacuum().unwrap();
    let error = run(
        &db,
        scan(equal(1), ScanDirection::Forward, &[2]),
        Some(built),
        &[0],
    )
    .unwrap_err();
    assert!(error.contains("cannot serve snapshot"), "{error}");
    assert_eq!(
        run(
            &db,
            scan(equal(2), ScanDirection::Forward, &[2]),
            None,
            &[0]
        )
        .unwrap()
        .len(),
        1
    );

    db.drop_index("idx_balance").unwrap();
    let error = run(
        &db,
        scan(equal(2), ScanDirection::Forward, &[2]),
        None,
        &[0],
    )
    .unwrap_err();
    assert!(error.contains("does not exist"), "{error}");
}

/// Bounds no entry can match return nothing rather than failing: a value longer than any
/// stored key, or of another type than the column. Bounds the index cannot address and
/// unknown indexes are errors, never panics.
#[test]
fn extreme_bounds_are_handled() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    let huge = equal_value(Value::String("x".repeat(10_000)));
    assert!(
        run(&db, scan(huge, ScanDirection::Forward, &[2]), None, &[0])
            .unwrap()
            .is_empty()
    );
    let other_type = equal_value(Value::Bytes(vec![0, 0xFF, 0]));
    assert!(run(
        &db,
        scan(other_type, ScanDirection::Backward, &[2]),
        None,
        &[0]
    )
    .unwrap()
    .is_empty());
    let two_columns = IndexBounds {
        prefix: vec![Value::Int64(1), Value::Int64(2)],
        ..IndexBounds::all()
    };
    let error = run(
        &db,
        scan(two_columns, ScanDirection::Forward, &[2]),
        None,
        &[0],
    )
    .unwrap_err();
    assert!(error.contains("key columns"), "{error}");
    let unknown = PhysicalPlan::IndexScan {
        index_id: 99,
        bounds: IndexBounds::all(),
        direction: ScanDirection::Forward,
        columns: vec![],
        limit: Some(1),
    };
    assert!(run(&db, unknown, None, &[]).is_err());
    assert!(!db.is_poisoned());
}

/// Equality on any single value.
fn equal_value(value: Value) -> IndexBounds {
    IndexBounds {
        prefix: vec![value],
        ..IndexBounds::all()
    }
}
