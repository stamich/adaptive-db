//! `CREATE INDEX` / `DROP INDEX`: builds over existing rows, publication by checkpoint, and
//! crashes at every step of the lifecycle.
mod common;

use adb_core::{Direction, IndexId, Row, RowId, Value};
use adb_engine::{
    index_catalog::{index_dir, IndexCatalog},
    Database, DbError, IndexColumn, IndexSpec, IndexState,
};
use tempfile::tempdir;

/// Entity of the accounts.
const ACCOUNT: u64 = 3;

/// `CREATE INDEX name ON account (balance) INCLUDE (owner)`.
fn spec(name: &str) -> IndexSpec {
    IndexSpec {
        name: name.into(),
        entity_id: ACCOUNT,
        columns: vec![IndexColumn {
            field_id: 2,
            direction: Direction::Asc,
        }],
        include: vec![3],
    }
}

/// Inserts `count` accounts starting at primary key `from`, one commit each.
fn accounts(db: &Database, from: i64, count: i64) {
    for pk in from..from + count {
        let mut tx = db.begin();
        tx.put(
            RowId::from_primary_key(ACCOUNT, pk),
            Row::new()
                .with_field(2, Value::Int64(pk % 50))
                .with_field(3, Value::String(format!("owner-{pk}"))),
        );
        db.commit(tx).unwrap();
    }
}

/// A build over existing rows equals the table, serves snapshots from its creation, keeps up
/// with later commits, and survives a reopen.
#[test]
fn create_index_builds_over_existing_rows() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    accounts(&db, -100, 300);
    let before = db.latest_committed_ts();
    let definition = db.create_index(spec("idx_balance")).unwrap();
    assert_eq!(definition.state, IndexState::Active);
    assert_eq!(definition.created_at_ts, before);
    let status = db.indexes();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].usable_from, before);
    assert_eq!(status[0].watermark, before);
    assert!(db.verify().unwrap().is_ok());

    accounts(&db, 500, 20);
    assert!(db.indexes()[0].watermark > before);
    assert!(db.verify().unwrap().is_ok());
    drop(db); // crash: the build was published by its checkpoint, later commits are replayed
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.indexes()[0].definition.name, "idx_balance");
    assert!(db.verify().unwrap().is_ok());
}

/// Names are unique, invalid specs are refused, a missing index cannot be dropped.
#[test]
fn definitions_are_checked() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_index(spec("idx_balance")).unwrap();
    assert!(matches!(
        db.create_index(spec("IDX_BALANCE")),
        Err(DbError::InvalidArgument(_))
    ));
    let mut duplicate = spec("dup");
    duplicate.include = vec![2];
    assert!(matches!(db.create_index(duplicate), Err(DbError::Index(_))));
    assert!(matches!(
        db.drop_index("missing"),
        Err(DbError::IndexNotFound(_))
    ));
    assert!(!db.is_poisoned());
}

/// A build that meets a row it cannot index fails without a trace.
#[test]
fn failed_builds_leave_nothing_behind() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.put(
        RowId::from_primary_key(ACCOUNT, 1),
        Row::new().with_field(2, Value::String("x".repeat(700))),
    );
    db.commit(tx).unwrap();
    assert!(db.create_index(spec("idx_balance")).is_err());
    assert!(db.indexes().is_empty());
    assert!(!db.is_poisoned());
    assert!(!index_dir(dir.path(), IndexId(0)).exists());
    accounts(&db, 10, 3);
}

/// Drop removes definition and files; the name can be reused (with a new id).
#[test]
fn drop_index_removes_definition_and_files() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    accounts(&db, 0, 50);
    let first = db.create_index(spec("idx_balance")).unwrap();
    assert!(index_dir(dir.path(), first.id).exists());
    let dropped = db.drop_index("idx_balance").unwrap();
    assert_eq!(dropped.id, first.id);
    assert!(db.indexes().is_empty());
    assert!(!index_dir(dir.path(), first.id).exists());
    let second = db.create_index(spec("idx_balance")).unwrap();
    assert_ne!(second.id, first.id);
    drop(db);
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.indexes()[0].definition.id, second.id);
    assert!(db.verify().unwrap().is_ok());
}

/// Crash during a build (files written, catalog never published): the next open removes the
/// files and the database has no index.
#[test]
fn crash_during_build_leaves_no_index() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        accounts(&db, 0, 10);
        db.close().unwrap();
    }
    let orphan = index_dir(dir.path(), IndexId(0));
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("entries.idx"), vec![0u8; 100]).unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert!(db.indexes().is_empty());
    assert!(!orphan.exists());
    let created = db.create_index(spec("idx_balance")).unwrap();
    assert_eq!(
        created.id,
        IndexId(0),
        "an unpublished build did not use up its id"
    );
}

/// Crash after a drop's checkpoint but before its files were removed: the next open removes
/// them; the catalog no longer names the index.
#[test]
fn crash_after_drop_checkpoint_removes_leftover_files() {
    let dir = tempdir().unwrap();
    let id = {
        let db = Database::open(dir.path()).unwrap();
        accounts(&db, 0, 10);
        let id = db.create_index(spec("idx_balance")).unwrap().id;
        db.close().unwrap();
        id
    };
    // Simulate the drop's checkpoint without the file removal.
    let mut catalog = IndexCatalog::load(dir.path()).unwrap();
    catalog.remove(id).unwrap();
    catalog.save(dir.path()).unwrap();
    assert!(index_dir(dir.path(), id).exists());
    let db = Database::open(dir.path()).unwrap();
    assert!(db.indexes().is_empty());
    assert!(!index_dir(dir.path(), id).exists());
    assert!(db.verify().unwrap().is_ok());
}

/// Snapshots before the creation are outside the range the index serves.
#[test]
fn indexes_serve_snapshots_from_their_creation() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    accounts(&db, 0, 5);
    let old = db.latest_committed_ts();
    accounts(&db, 5, 5);
    db.create_index(spec("idx_balance")).unwrap();
    let status = &db.indexes()[0];
    assert!(status.usable_from > old);
    assert!(status.usable_from <= status.watermark);
    assert_eq!(status.watermark, db.latest_committed_ts());
}
