//! The global commit order: every data and catalog commit gets the next sequence number, the
//! number is part of the log record, and it survives restarts.
mod common;

use adb_core::{CommitSeq, RowId, SchemaEpoch, ValueType};
use adb_engine::{Database, EntitySpec, NewField};
use adb_wal::{read_all, WalRecord};
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

/// A new database has head 0; each commit returns the next number and moves the head to it.
#[test]
fn data_commits_take_consecutive_sequence_numbers() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.latest_commit_seq(), CommitSeq::ZERO);
    for expected in 1..=3u64 {
        let mut tx = db.begin();
        tx.put(RowId(expected as u128), row_with_i64(1));
        let result = db.commit(tx).unwrap();
        assert_eq!(result.commit_seq, CommitSeq(expected));
        assert_eq!(db.latest_commit_seq(), result.commit_seq);
    }
}

/// DDL and DML share one sequence, in the order they were committed: 1 data, 2 DDL, 3 data.
#[test]
fn catalog_and_data_commits_share_one_order() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    assert_eq!(db.latest_commit_seq(), CommitSeq(1));
    db.create_entity(table()).unwrap();
    assert_eq!(db.latest_commit_seq(), CommitSeq(2));
    let mut tx = db.begin();
    tx.put(RowId(2), row_with_i64(2));
    assert_eq!(db.commit(tx).unwrap().commit_seq, CommitSeq(3));
}

/// A read-only transaction commits nothing: it reports the head and does not move it.
#[test]
fn read_only_commits_do_not_consume_a_number() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    let mut reader = db.begin();
    db.get_in_tx(&mut reader, RowId(1)).unwrap();
    let result = db.commit(reader).unwrap();
    assert_eq!(result.commit_seq, CommitSeq(1));
    assert_eq!(db.latest_commit_seq(), CommitSeq(1));
}

/// The log holds the sequence number and the catalog epoch of every commit.
#[test]
fn the_log_records_sequence_numbers_and_epochs() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId(1), 1);
        db.create_entity(table()).unwrap();
        put(&db, RowId(2), 2);
        db.close().unwrap();
    }
    let commits: Vec<(u64, u64)> = read_all(dir.path().join("wal"))
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.record {
            WalRecord::CommitV2 {
                commit_seq,
                schema_epoch,
                ..
            } => Some((commit_seq.0, schema_epoch.0)),
            _ => None,
        })
        .collect();
    assert_eq!(commits, vec![(1, 0), (2, 1), (3, 1)]);
}

/// The head and the numbering continue after a restart, with and without a checkpoint.
#[test]
fn numbering_survives_restarts() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId(1), 1);
        put(&db, RowId(2), 2);
        db.checkpoint().unwrap();
        put(&db, RowId(3), 3);
        // Dropped without close: the third commit is only in the log tail.
    }
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.latest_commit_seq(), CommitSeq(3));
    assert_eq!(db.schema_epoch(), SchemaEpoch(0));
    let mut tx = db.begin();
    tx.put(RowId(4), row_with_i64(4));
    assert_eq!(db.commit(tx).unwrap().commit_seq, CommitSeq(4));
    db.close().unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.latest_commit_seq(), CommitSeq(4));
}

/// A log written by 2.4.0 (data format 3, commit records without numbers) opens in place: its
/// commits are numbered by timestamp and new commits continue above them.
#[test]
fn logs_without_sequence_numbers_are_numbered_by_timestamp() {
    use adb_core::{CommitTs, TxId};
    use adb_engine::format::stamp;
    use adb_wal::SegmentedWalWriter;

    let dir = tempdir().unwrap();
    let wal = dir.path().join("wal");
    std::fs::create_dir_all(&wal).unwrap();
    {
        let mut writer = SegmentedWalWriter::open(&wal, 1 << 20).unwrap();
        for tx in 1..=2u64 {
            for record in [
                WalRecord::Begin {
                    tx_id: TxId(tx),
                    snapshot_ts: CommitTs(tx - 1),
                },
                WalRecord::Put {
                    tx_id: TxId(tx),
                    row_id: RowId(tx as u128),
                    value: row_with_i64(tx as i64),
                },
                WalRecord::Commit {
                    tx_id: TxId(tx),
                    commit_ts: CommitTs(tx),
                },
            ] {
                writer.append(&record).unwrap();
            }
        }
        writer.sync().unwrap();
    }
    stamp(dir.path(), 3).unwrap();

    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.latest_commit_seq(), CommitSeq(2));
    assert_eq!(db.latest_committed_ts(), CommitTs(2));
    let mut tx = db.begin();
    tx.put(RowId(3), row_with_i64(3));
    let result = db.commit(tx).unwrap();
    assert_eq!(result.commit_seq, CommitSeq(3));
    db.close().unwrap();

    // The head is recomputed from the log when the projections are rebuilt.
    let db = Database::rebuild_projections(dir.path(), Default::default()).unwrap();
    assert_eq!(db.latest_commit_seq(), CommitSeq(3));
}

/// A log whose sequence numbers do not increase is refused as corrupt instead of being
/// replayed.
#[test]
fn a_repeated_sequence_number_is_corruption() {
    use adb_core::{CommitTs, SchemaEpoch, TxId};
    use adb_wal::SegmentedWalWriter;

    let dir = tempdir().unwrap();
    let wal = dir.path().join("wal");
    std::fs::create_dir_all(&wal).unwrap();
    {
        let mut writer = SegmentedWalWriter::open(&wal, 1 << 20).unwrap();
        for tx in 1..=2u64 {
            for record in [
                WalRecord::Begin {
                    tx_id: TxId(tx),
                    snapshot_ts: CommitTs(tx - 1),
                },
                WalRecord::Put {
                    tx_id: TxId(tx),
                    row_id: RowId(tx as u128),
                    value: row_with_i64(1),
                },
                WalRecord::CommitV2 {
                    tx_id: TxId(tx),
                    commit_ts: CommitTs(tx),
                    commit_seq: CommitSeq(7), // the same number twice
                    schema_epoch: SchemaEpoch(0),
                },
            ] {
                writer.append(&record).unwrap();
            }
        }
        writer.sync().unwrap();
    }
    adb_engine::format::stamp(dir.path(), adb_engine::format::DATA_FORMAT_VERSION).unwrap();
    let error = Database::open(dir.path()).err().expect("refused");
    assert!(error.is_corruption(), "{error:?}");
}
