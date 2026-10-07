//! Version-store behaviour: temporal lookup, idempotence and distinct-row enumeration.
use adb_core::{CommitTs, KeyRange, Lsn, Row, RowId, Value};
use adb_storage::{HistoricalVersion, PersistentVersionStore};
use tempfile::tempdir;

/// Version `[begin, end)` with field 1 = `value`.
fn version(begin: u64, end: u64, value: i64) -> HistoricalVersion {
    HistoricalVersion {
        begin_ts: CommitTs(begin),
        end_ts: CommitTs(end),
        value: Some(Row::new().with_field(1, Value::Int64(value))),
    }
}

/// Versions survive a reopen and are visible exactly within their interval.
#[test]
fn persistent_version_store_survives_restart() {
    let dir = tempdir().unwrap();
    {
        let store = PersistentVersionStore::open(dir.path(), 64).unwrap();
        store
            .put_at_lsn(RowId(7), &version(10, 20, 100), Lsn(10))
            .unwrap();
        store
            .put_at_lsn(RowId(7), &version(20, 30, 200), Lsn(20))
            .unwrap();
        store.flush().unwrap();
    }
    let store = PersistentVersionStore::open(dir.path(), 64).unwrap();
    assert_eq!(
        store.get_at(RowId(7), CommitTs(15)).unwrap(),
        Some(version(10, 20, 100))
    );
    assert_eq!(
        store.get_at(RowId(7), CommitTs(25)).unwrap(),
        Some(version(20, 30, 200))
    );
    assert_eq!(store.get_at(RowId(7), CommitTs(30)).unwrap(), None);
    assert_eq!(store.get_at(RowId(7), CommitTs(5)).unwrap(), None);
    assert_eq!(store.history(RowId(7)).unwrap().len(), 2);
}

/// Storing the same version repeatedly keeps one copy (idempotent replay).
#[test]
fn replaying_a_version_is_idempotent() {
    let dir = tempdir().unwrap();
    let store = PersistentVersionStore::open(dir.path(), 64).unwrap();
    for _ in 0..3 {
        store
            .put_at_lsn(RowId(1), &version(1, 2, 1), Lsn(1))
            .unwrap();
    }
    assert_eq!(store.history(RowId(1)).unwrap().len(), 1);
}

/// Distinct-row enumeration pages through one entity regardless of history length.
#[test]
fn row_ids_enumerates_distinct_rows_per_entity() {
    let dir = tempdir().unwrap();
    let store = PersistentVersionStore::open(dir.path(), 64).unwrap();
    for entity in 1..=2u64 {
        for pk in 0..40u64 {
            for begin in 0..5u64 {
                store
                    .put_at_lsn(
                        RowId::compose(entity, pk),
                        &version(begin, begin + 1, 0),
                        Lsn(1),
                    )
                    .unwrap();
            }
        }
    }
    let range = KeyRange::entity(2);
    let first = store.row_ids(&range, None, 25).unwrap();
    assert_eq!(first.len(), 25);
    let rest = store.row_ids(&range, first.last().copied(), 100).unwrap();
    assert_eq!(rest.len(), 15);
    assert!(first.iter().chain(&rest).all(|row| row.entity_id() == 2));
}
