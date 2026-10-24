//! Indexes as projections of the log: every commit maintains them, recovery replays them,
//! rebuilding the projections rebuilds them, and they always equal their table.
mod common;

use adb_core::{CommitTs, Direction, Row, RowId, Value};
use adb_engine::{index_catalog::IndexCatalog, Database, DatabaseOptions, DbError};
use adb_index::{IndexColumn, IndexError, IndexSpec, IndexState};
use tempfile::tempdir;

/// Entity of the indexed table.
const ENTITY: u64 = 5;

/// Writes a catalog with one active index on (field 2) INCLUDE (field 3) of [`ENTITY`] into a
/// fresh directory, as `CREATE INDEX` on an empty table would leave it.
fn with_index(dir: &std::path::Path) {
    let mut catalog = IndexCatalog::default();
    catalog
        .add(
            IndexSpec {
                name: "idx_balance".into(),
                entity_id: ENTITY,
                columns: vec![IndexColumn {
                    field_id: 2,
                    direction: Direction::Asc,
                }],
                include: vec![3],
            },
            IndexState::Active,
            CommitTs(0),
        )
        .unwrap();
    catalog.save(dir).unwrap();
}

/// An account row: balance (field 2), owner (field 3), note (field 4, not indexed).
fn account(balance: i64, owner: &str, note: i64) -> Row {
    Row::new()
        .with_field(2, Value::Int64(balance))
        .with_field(3, Value::String(owner.into()))
        .with_field(4, Value::Int64(note))
}

/// Applies a deterministic mix of inserts, key changes, unindexed changes and deletes.
fn workload(db: &Database, rounds: i64) {
    for round in 0..rounds {
        let mut tx = db.begin();
        for pk in 0..20 {
            let id = RowId::from_primary_key(ENTITY, pk - 10);
            match (round + pk) % 5 {
                0 => tx.delete(id),
                1 => tx.put(id, account(pk * round % 7, "same", round)),
                _ => tx.put(
                    id,
                    account(pk % 3, &format!("owner-{}", (pk + round) % 4), 0),
                ),
            }
        }
        // A row of another entity never touches the index.
        tx.put(
            RowId::from_primary_key(ENTITY + 1, round),
            account(1, "other", 0),
        );
        db.commit(tx).unwrap();
    }
}

/// Every commit keeps the index equal to its table.
#[test]
fn commits_maintain_the_index() {
    let dir = tempdir().unwrap();
    with_index(dir.path());
    let db = Database::open(dir.path()).unwrap();
    workload(&db, 25);
    let report = db.verify().unwrap();
    assert!(report.is_ok(), "{:?}", report.errors);
}

/// After a crash (no checkpoint) recovery replays the log into the index; after a checkpoint
/// the index is read back from disk; both equal the table.
#[test]
fn recovery_replays_and_checkpoints_publish_the_index() {
    let dir = tempdir().unwrap();
    with_index(dir.path());
    {
        let db = Database::open(dir.path()).unwrap();
        workload(&db, 10);
        db.checkpoint().unwrap();
        workload(&db, 7); // never checkpointed: lost from the projections, kept in the log
    }
    let db = Database::open(dir.path()).unwrap();
    let report = db.verify().unwrap();
    assert!(report.is_ok(), "{:?}", report.errors);
    workload(&db, 3);
    assert!(db.verify().unwrap().is_ok());
}

/// Rebuilding the projections rebuilds the index from the whole log and keeps its definition.
#[test]
fn rebuild_projections_rebuilds_the_index() {
    let dir = tempdir().unwrap();
    with_index(dir.path());
    {
        let db = Database::open(dir.path()).unwrap();
        workload(&db, 12);
        db.close().unwrap();
    }
    // Damage the index file: the database refuses it, a rebuild repairs it.
    let index_file = std::fs::read_dir(dir.path().join("indexes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap()
        .join("entries.idx");
    let mut bytes = std::fs::read(&index_file).unwrap();
    for byte in bytes.iter_mut().skip(64).step_by(97) {
        *byte ^= 0xA5;
    }
    std::fs::write(&index_file, bytes).unwrap();
    let damaged = Database::open(dir.path()).and_then(|db| db.verify().map(|report| (db, report)));
    match damaged {
        Err(error) => assert!(error.is_corruption(), "{error:?}"),
        Ok((_, report)) => assert!(!report.is_ok(), "the damage is reported"),
    }
    let db = Database::rebuild_projections(dir.path(), DatabaseOptions::default()).unwrap();
    let report = db.verify().unwrap();
    assert!(report.is_ok(), "{:?}", report.errors);
    workload(&db, 2);
    assert!(db.verify().unwrap().is_ok());
}

/// A row whose key cannot be indexed is refused before it is logged; the database stays
/// healthy and keeps working.
#[test]
fn unindexable_rows_are_refused_before_logging() {
    let dir = tempdir().unwrap();
    with_index(dir.path());
    let db = Database::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.put(
        RowId::from_primary_key(ENTITY, 1),
        Row::new().with_field(2, Value::String("k".repeat(600))),
    );
    match db.commit(tx) {
        Err(DbError::Index(IndexError::KeyTooLarge { .. })) => {}
        other => panic!("expected KeyTooLarge, got {other:?}"),
    }
    assert!(!db.is_poisoned());
    workload(&db, 2);
    assert!(db.verify().unwrap().is_ok());
    assert!(db
        .get(RowId::from_primary_key(ENTITY, 1))
        .unwrap()
        .is_some());
}

/// Vacuum trims ended index entries together with tombstones.
#[test]
fn vacuum_trims_index_history() {
    let dir = tempdir().unwrap();
    with_index(dir.path());
    let db = Database::open(dir.path()).unwrap();
    workload(&db, 8);
    let report = db.vacuum().unwrap();
    assert!(report.index_entries_removed > 0, "{report:?}");
    assert!(db.verify().unwrap().is_ok());
}
