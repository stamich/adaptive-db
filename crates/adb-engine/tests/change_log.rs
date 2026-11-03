//! The engine appends one change-log envelope per commit, in commit order, before the commit is
//! published.
mod common;

use adb_change_log::{ChangePayload, SchemaChangeMarker};
use adb_core::{CommitSeq, RowId, ValueType};
use adb_engine::{Database, EntitySpec, NewField};
use common::{put, row_with_i64};
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

/// Every commit has its envelope, with the images of the rows it changed.
#[test]
fn every_commit_has_an_envelope_with_row_images() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 10); // insert
    put(&db, RowId(1), 11); // update
    let mut tx = db.begin();
    tx.delete(RowId(1));
    let last = db.commit(tx).unwrap();

    let log = db.change_log();
    assert_eq!(log.head(), Some(last.commit_seq));
    let envelopes = log.read_after(CommitSeq::ZERO, 10).unwrap();
    assert_eq!(envelopes.len(), 3);
    let images: Vec<(bool, bool)> = envelopes
        .iter()
        .map(|envelope| match &envelope.payload {
            ChangePayload::Data { rows, .. } => {
                assert_eq!(rows.len(), 1);
                (rows[0].before.is_some(), rows[0].after.is_some())
            }
            other => panic!("expected a data payload, got {other:?}"),
        })
        .collect();
    assert_eq!(images, [(false, true), (true, true), (true, false)]);
    for (index, envelope) in envelopes.iter().enumerate() {
        assert_eq!(envelope.commit_seq, CommitSeq(index as u64 + 1));
    }
}

/// DDL and DML appear in one order, with the schema marker of the DDL commit.
#[test]
fn ddl_and_dml_share_the_change_log_order() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    let schema = db.create_entity(table()).unwrap();
    put(&db, RowId(2), 2);
    let envelopes = db.change_log().read_after(CommitSeq::ZERO, 10).unwrap();
    assert_eq!(envelopes.len(), 3);
    assert!(matches!(envelopes[0].payload, ChangePayload::Data { .. }));
    match &envelopes[1].payload {
        ChangePayload::Schema { changes, .. } => assert_eq!(
            changes,
            &[SchemaChangeMarker::Schema {
                entity_id: schema.entity_id,
                schema_version: schema.version
            }]
        ),
        other => panic!("expected a schema payload, got {other:?}"),
    }
    assert!(matches!(envelopes[2].payload, ChangePayload::Data { .. }));
    assert_eq!(envelopes[1].schema_epoch, db.schema_epoch());
}

/// A rejected commit (conflict) leaves no envelope and no gap.
#[test]
fn a_conflicting_commit_leaves_nothing_behind() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    let mut a = db.begin();
    let mut b = db.begin();
    a.put(RowId(1), row_with_i64(2));
    b.put(RowId(1), row_with_i64(3));
    db.commit(a).unwrap();
    assert!(db.commit(b).is_err());
    let envelopes = db.change_log().read_after(CommitSeq::ZERO, 10).unwrap();
    let seqs: Vec<u64> = envelopes.iter().map(|e| e.commit_seq.0).collect();
    assert_eq!(seqs, [1, 2]);
}

/// A checkpoint makes the change log durable; it is intact after a restart.
#[test]
fn the_change_log_survives_a_clean_restart() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        for i in 1..=5 {
            put(&db, RowId(i), i as i64);
        }
        db.close().unwrap();
    }
    let db = Database::open(dir.path()).unwrap();
    let envelopes = db.change_log().read_after(CommitSeq::ZERO, 10).unwrap();
    assert_eq!(envelopes.len(), 5);
    put(&db, RowId(6), 6);
    assert_eq!(db.change_log().head(), Some(CommitSeq(6)));
}
