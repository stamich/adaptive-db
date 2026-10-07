//! Entity-scoped streaming scans, tombstone vacuum, and their interaction with history.
mod common;

use adb_core::{CommitTs, RowId};
use adb_engine::{Database, DbError, IsolationLevel};
use adb_execution::PhysicalPlan;

/// Entity scan of `entity` reading no fields (only keys are inspected).
fn entity_scan(entity: u64) -> PhysicalPlan {
    PhysicalPlan::EntityScan {
        entity_id: entity,
        columns: Vec::new(),
    }
}

/// Full scan of every entity reading no fields.
fn full_scan() -> PhysicalPlan {
    PhysicalPlan::Scan {
        columns: Vec::new(),
    }
}
use tempfile::tempdir;

use common::{delete, put, read_i64, row_with_i64, value};

/// Executes `plan` at `at`, or at the latest snapshot.
fn cursor(db: &Database, plan: PhysicalPlan, at: Option<CommitTs>) -> adb_execution::QueryCursor {
    match at {
        Some(ts) => db.execute_at(plan, ts).unwrap(),
        None => db.execute(plan).unwrap(),
    }
}

/// Number of rows a plan returns.
fn count(db: &Database, plan: PhysicalPlan, at: Option<CommitTs>) -> usize {
    let mut cursor = cursor(db, plan, at);
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch().unwrap() {
        rows += batch.len();
    }
    rows
}

/// Primary keys returned by an entity scan, in order.
fn entity_keys(db: &Database, entity: u64, at: Option<CommitTs>) -> Vec<u64> {
    let mut cursor = cursor(db, entity_scan(entity), at);
    let mut keys = Vec::new();
    while let Some(batch) = cursor.next_batch().unwrap() {
        keys.extend(
            batch
                .row_ids
                .iter()
                .flatten()
                .map(|row_id| row_id.primary_key()),
        );
    }
    keys
}

/// An entity scan streams exactly that entity; a full scan sees every entity.
#[test]
fn entity_scan_streams_only_its_entity() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for entity in 1..=3u64 {
        for pk in 0..1500u64 {
            put(&db, RowId::compose(entity, pk), pk as i64);
        }
    }
    let keys = entity_keys(&db, 2, None);
    assert_eq!(keys, (0..1500).collect::<Vec<_>>());
    assert_eq!(count(&db, full_scan(), None), 4500);
}

/// Repeated updates no longer leak heap space (regression for the 2.0.2 append-only heap).
#[test]
fn update_churn_keeps_the_current_heap_bounded() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for row in 0..50u128 {
        put(&db, RowId(row), 0);
    }
    let baseline = db.storage_stats().unwrap().current_heap_pages;
    for round in 1..100 {
        for row in 0..50u128 {
            put(&db, RowId(row), round);
        }
    }
    assert_eq!(db.storage_stats().unwrap().current_heap_pages, baseline);
}

/// Vacuum removes tombstones while point reads and scans at older snapshots still see the rows.
#[test]
fn vacuum_removes_tombstones_and_history_stays_readable() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for pk in 0..100u64 {
        put(&db, RowId::compose(1, pk), pk as i64);
    }
    let before_deletes = db.latest_committed_ts();
    for pk in 0..100u64 {
        delete(&db, RowId::compose(1, pk));
    }
    assert_eq!(db.storage_stats().unwrap().current_tombstones, 100);

    let report = db.vacuum().unwrap();
    assert_eq!(report.tombstones_removed, 100);
    let stats = db.storage_stats().unwrap();
    assert_eq!(stats.current_rows, 0);
    assert_eq!(stats.current_tombstones, 0);

    // Point reads and *scans* at the old snapshot still see every deleted row.
    assert_eq!(
        read_i64(
            &db.get_at(RowId::compose(1, 42), before_deletes)
                .unwrap()
                .unwrap()
        ),
        42
    );
    assert_eq!(entity_keys(&db, 1, Some(before_deletes)).len(), 100);
    assert!(entity_keys(&db, 1, None).is_empty());
}

/// Vacuum survives a restart together with the scan rule that depends on it.
#[test]
fn vacuum_is_persisted_by_checkpoint() {
    let dir = tempdir().unwrap();
    let before_delete;
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId(1), 1);
        before_delete = db.latest_committed_ts();
        delete(&db, RowId(1));
        db.vacuum().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.storage_stats().unwrap().current_rows, 0);
    assert_eq!(count(&db, full_scan(), Some(before_delete)), 1);
}

/// A tombstone that a live transaction may still conflict with is kept, so its stale write
/// is still rejected.
#[test]
fn vacuum_keeps_tombstones_needed_by_active_transactions() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    let mut stale = db.begin_with(IsolationLevel::Snapshot);
    delete(&db, RowId(1));

    assert_eq!(db.vacuum().unwrap().tombstones_removed, 0);
    stale.put(RowId(1), row_with_i64(99));
    assert!(matches!(
        db.commit(stale),
        Err(DbError::TransactionConflict(_))
    ));
    assert_eq!(value(&db, RowId(1)), None);
    assert_eq!(db.vacuum().unwrap().tombstones_removed, 1);
}
