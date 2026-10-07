//! Current-store behaviour: persistence, space reuse, removal and range scans.
use adb_core::{CommitTs, KeyRange, Lsn, Row, RowId, Value};
use adb_storage::{CurrentRecord, PersistentCurrentStore};
use tempfile::tempdir;

fn record(ts: u64, value: i64) -> CurrentRecord {
    CurrentRecord {
        commit_ts: CommitTs(ts),
        value: Some(Row::new().with_field(1, Value::Int64(value))),
    }
}

#[test]
fn current_store_survives_reopen() {
    let dir = tempdir().unwrap();
    {
        let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
        store.put_at_lsn(RowId(9), &record(7, 42), Lsn(1)).unwrap();
        store.flush().unwrap();
    }
    let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
    assert_eq!(store.get(RowId(9)).unwrap(), Some(record(7, 42)));
}

/// Regression for the 2.0.2 leak: every update appended a new heap tuple and never freed the
/// old one. Repeated updates must now keep the heap at a constant size.
#[test]
fn updates_do_not_grow_the_heap() {
    let dir = tempdir().unwrap();
    let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
    for row in 0..100u128 {
        store.put_at_lsn(RowId(row), &record(1, 0), Lsn(1)).unwrap();
    }
    let baseline = store.heap_page_count();
    for round in 2..200u64 {
        for row in 0..100u128 {
            store
                .put_at_lsn(RowId(row), &record(round, round as i64), Lsn(round))
                .unwrap();
        }
    }
    assert_eq!(store.heap_page_count(), baseline);
    assert_eq!(store.get(RowId(50)).unwrap(), Some(record(199, 199)));
}

#[test]
fn removed_rows_free_their_space_and_disappear() {
    let dir = tempdir().unwrap();
    let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
    for row in 0..1000u128 {
        store.put_at_lsn(RowId(row), &record(1, 1), Lsn(1)).unwrap();
    }
    let pages = store.heap_page_count();
    for row in 0..1000u128 {
        assert!(store.remove(RowId(row), Lsn(2)).unwrap());
    }
    assert!(!store.remove(RowId(1), Lsn(2)).unwrap());
    assert!(store.entries().unwrap().is_empty());
    for row in 0..1000u128 {
        store.put_at_lsn(RowId(row), &record(3, 3), Lsn(3)).unwrap();
    }
    assert_eq!(store.heap_page_count(), pages, "freed space is reused");
}

/// Free space found on disk is reused after a reopen as well.
#[test]
fn free_space_map_is_rebuilt_on_open() {
    let dir = tempdir().unwrap();
    let pages;
    {
        let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
        for row in 0..1000u128 {
            store.put_at_lsn(RowId(row), &record(1, 1), Lsn(1)).unwrap();
        }
        for row in 0..1000u128 {
            store.remove(RowId(row), Lsn(2)).unwrap();
        }
        pages = store.heap_page_count();
        store.flush().unwrap();
    }
    let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
    for row in 0..1000u128 {
        store.put_at_lsn(RowId(row), &record(3, 3), Lsn(3)).unwrap();
    }
    assert_eq!(store.heap_page_count(), pages);
}

#[test]
fn entity_scan_is_paged_and_confined_to_the_entity() {
    let dir = tempdir().unwrap();
    let store = PersistentCurrentStore::open(dir.path(), 64).unwrap();
    for entity in 1..=3u64 {
        for pk in 0..250u64 {
            store
                .put_at_lsn(RowId::compose(entity, pk), &record(1, pk as i64), Lsn(1))
                .unwrap();
        }
    }
    let range = KeyRange::entity(2);
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = store.scan(&range, after, 64).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 64);
        after = page.last().map(|(row_id, _)| *row_id);
        seen.extend(page.into_iter().map(|(row_id, _)| row_id));
    }
    assert_eq!(seen.len(), 250);
    assert!(seen.iter().all(|row_id| row_id.entity_id() == 2));
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
}
