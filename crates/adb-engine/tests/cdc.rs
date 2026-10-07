//! Native change-data capture.
mod common;

use adb_core::RowId;
use adb_engine::{ChangeCursor, ChangeFilter, ChangeKind, Database, DbError};
use tempfile::tempdir;

use common::{delete, put, read_i64, row_with_i64};

fn all_changes(db: &Database, from: ChangeCursor) -> adb_engine::ChangeBatch {
    db.read_changes(from, usize::MAX, &ChangeFilter::all())
        .unwrap()
}

#[test]
fn insert_update_delete_carry_before_and_after_images() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let row = RowId::compose(1, 7);
    let t1 = put(&db, row, 10);
    let t2 = put(&db, row, 20);
    let t3 = delete(&db, row);

    let batch = all_changes(&db, ChangeCursor::BEGINNING);
    assert_eq!(batch.events.len(), 3);
    let kinds: Vec<_> = batch.events.iter().map(|e| e.changes[0].kind).collect();
    assert_eq!(
        kinds,
        [ChangeKind::Insert, ChangeKind::Update, ChangeKind::Delete]
    );
    assert_eq!(
        batch.events.iter().map(|e| e.commit_ts).collect::<Vec<_>>(),
        [t1, t2, t3]
    );

    let update = &batch.events[1].changes[0];
    assert_eq!(update.entity_id(), 1);
    assert_eq!(update.primary_key(), 7);
    assert_eq!(read_i64(update.before.as_ref().unwrap()), 10);
    assert_eq!(read_i64(update.after.as_ref().unwrap()), 20);
    let removal = &batch.events[2].changes[0];
    assert_eq!(read_i64(removal.before.as_ref().unwrap()), 20);
    assert!(removal.after.is_none());

    // Commit order is strictly increasing in log position, and each event's cursor resumes
    // exactly at the next one.
    assert!(batch
        .events
        .windows(2)
        .all(|w| w[0].commit_lsn < w[1].commit_lsn));
    let resumed = all_changes(&db, batch.events[0].cursor_after);
    assert_eq!(resumed.events, batch.events[1..]);
    assert_eq!(batch.events[2].cursor_after, batch.next);
}

/// One multi-row transaction is one event; re-inserting a deleted row is an insert.
#[test]
fn transactions_are_atomic_events_and_reinsert_is_insert() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    delete(&db, RowId(1));
    let start = db.change_feed_end();

    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(2));
    tx.put(RowId(2), row_with_i64(3));
    tx.delete(RowId(3)); // never existed: not a visible change
    db.commit(tx).unwrap();

    let batch = all_changes(&db, start);
    assert_eq!(batch.events.len(), 1);
    let changes = &batch.events[0].changes;
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Insert));
}

/// Paging with `next` delivers every event exactly once, in order.
#[test]
fn paging_with_the_returned_cursor_is_gap_free_and_duplicate_free() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for i in 0..37 {
        put(&db, RowId(i), i as i64);
    }
    let mut cursor = ChangeCursor::BEGINNING;
    let mut seen = Vec::new();
    loop {
        let batch = db.read_changes(cursor, 5, &ChangeFilter::all()).unwrap();
        if batch.events.is_empty() {
            assert_eq!(batch.next, cursor, "an empty page does not move the cursor");
            break;
        }
        seen.extend(batch.events.iter().map(|e| e.changes[0].row_id.0));
        cursor = batch.next;
    }
    assert_eq!(seen, (0..37).collect::<Vec<_>>());
    assert_eq!(cursor, db.change_feed_end());
}

#[test]
fn entity_filter_skips_other_entities_but_still_advances() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId::compose(1, 1), 1);
    put(&db, RowId::compose(2, 1), 2);
    put(&db, RowId::compose(1, 2), 3);
    put(&db, RowId::compose(3, 1), 4);

    let batch = db
        .read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([1]))
        .unwrap();
    assert_eq!(batch.events.len(), 2);
    assert!(batch
        .events
        .iter()
        .all(|e| e.changes.iter().all(|c| c.entity_id() == 1)));
    assert_eq!(batch.next, db.change_feed_end());
}

/// Consumer offsets are durable; the feed survives restarts (it *is* the log).
#[test]
fn named_consumer_resumes_after_restart() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId(1), 1);
        put(&db, RowId(2), 2);
        let batch = all_changes(&db, ChangeCursor::BEGINNING);
        assert_eq!(batch.events.len(), 2);
        db.commit_consumer_offset("search-indexer", batch.next)
            .unwrap();
        put(&db, RowId(3), 3);
    } // crash
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(4), 4);
    let cursor = db.consumer_offset("search-indexer").unwrap();
    let batch = all_changes(&db, cursor);
    let rows: Vec<_> = batch.events.iter().map(|e| e.changes[0].row_id.0).collect();
    assert_eq!(rows, [3, 4]);
    assert!(db.consumer_offset("unknown").is_none());
}

#[test]
fn invalid_cursors_are_rejected() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    put(&db, RowId(2), 2);
    let end = db.change_feed_end();

    let beyond = ChangeCursor(adb_core::Lsn(end.0 .0 + 1));
    assert!(matches!(
        db.read_changes(beyond, 1, &ChangeFilter::all()),
        Err(DbError::InvalidArgument(_))
    ));
    let mid_frame = ChangeCursor(adb_core::Lsn(3));
    assert!(matches!(
        db.read_changes(mid_frame, 1, &ChangeFilter::all()),
        Err(DbError::InvalidArgument(_))
    ));
    assert!(matches!(
        db.commit_consumer_offset("x", beyond),
        Err(DbError::InvalidArgument(_))
    ));
    assert!(matches!(
        db.commit_consumer_offset("", end),
        Err(DbError::InvalidArgument(_))
    ));
}

/// Pruned logs (possible only for databases created by Milestone 2.0.2) are reported, not
/// silently skipped.
#[test]
fn reading_before_the_retained_log_reports_truncation() {
    let dir = tempdir().unwrap();
    let options = adb_engine::DatabaseOptions {
        log_segment_bytes: 1024,
        ..Default::default()
    };
    {
        let db = Database::open_with(dir.path(), options).unwrap();
        for i in 0..100 {
            put(&db, RowId(i), i as i64);
        }
        db.close().unwrap();
    }
    std::fs::remove_file(dir.path().join("wal").join("0000000000000000.wal")).unwrap();
    let db = Database::open_with(dir.path(), options).unwrap();
    assert!(matches!(
        db.read_changes(ChangeCursor::BEGINNING, 1, &ChangeFilter::all()),
        Err(DbError::ChangeLogTruncated { .. })
    ));
}
