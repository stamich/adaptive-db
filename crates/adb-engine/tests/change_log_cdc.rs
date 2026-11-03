//! The change feed (CDC, read from the WAL) and the change log (read from its own files) describe
//! the same commits: same order, same numbers, same row images.
mod common;

use adb_change_log::ChangePayload;
use adb_core::{CommitSeq, RowId, ValueType};
use adb_engine::{ChangeCursor, ChangeFilter, Database, EntitySpec, NewField};
use common::{delete, put};
use tempfile::tempdir;

/// `CREATE TABLE t (id BIGINT PRIMARY KEY)`.
fn table() -> EntitySpec {
    EntitySpec {
        name: "t".into(),
        fields: vec![NewField {
            name: "id".into(),
            value_type: ValueType::Int64,
            nullable: false,
            default: None,
            references: None,
            id: None,
        }],
        primary_key: "id".into(),
        entity_id: None,
    }
}

/// Every CDC event has an envelope with the same sequence number, timestamp and row images;
/// commits without a feed-visible change (index DDL) are the only envelopes without an event.
#[test]
fn cdc_events_and_envelopes_agree() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let row = RowId::from_primary_key(9, 5);
    put(&db, row, 1);
    put(&db, row, 2);
    db.create_entity(table()).unwrap();
    delete(&db, row);
    put(&db, RowId::from_primary_key(9, 6), 3);

    let events = db
        .read_changes(ChangeCursor::BEGINNING, usize::MAX, &ChangeFilter::all())
        .unwrap()
        .events;
    let envelopes = db.change_log().read_after(CommitSeq::ZERO, 100).unwrap();
    assert_eq!(events.len(), envelopes.len());
    for (event, envelope) in events.iter().zip(&envelopes) {
        assert_eq!(event.commit_seq, envelope.commit_seq);
        assert_eq!(event.commit_ts, envelope.commit_ts);
        match &envelope.payload {
            ChangePayload::Data { tx_id, rows } => {
                assert_eq!(*tx_id, event.tx_id);
                assert_eq!(rows.len(), event.changes.len());
                for (change, logged) in event.changes.iter().zip(rows) {
                    assert_eq!(change.row_id, logged.row_id);
                    assert_eq!(change.before, logged.before);
                    assert_eq!(change.after, logged.after);
                }
            }
            ChangePayload::Schema { tx_id, .. } => {
                assert_eq!(*tx_id, event.tx_id);
                assert!(!event.catalog.is_empty());
            }
        }
    }
    let seqs: Vec<u64> = events.iter().map(|e| e.commit_seq.0).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
}
