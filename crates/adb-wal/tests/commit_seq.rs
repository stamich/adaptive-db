//! The sequenced commit record (data format 4): written, read back, mixed with the commit
//! record of older logs, and appended without disturbing the encoding of earlier kinds.
use adb_core::{CommitSeq, CommitTs, SchemaEpoch, TxId};
use adb_wal::{read_all, SegmentedWalWriter, WalRecord};
use tempfile::tempdir;

/// A commit record carries its sequence number and epoch through the log, and an old-style
/// commit record in the same log is still read.
#[test]
fn sequenced_and_legacy_commits_share_a_log() {
    let dir = tempdir().unwrap();
    let records = vec![
        WalRecord::Begin {
            tx_id: TxId(1),
            snapshot_ts: CommitTs(0),
        },
        WalRecord::Commit {
            tx_id: TxId(1),
            commit_ts: CommitTs(1),
        },
        WalRecord::Begin {
            tx_id: TxId(2),
            snapshot_ts: CommitTs(1),
        },
        WalRecord::CommitV2 {
            tx_id: TxId(2),
            commit_ts: CommitTs(2),
            commit_seq: CommitSeq(2),
            schema_epoch: SchemaEpoch(5),
        },
    ];
    let mut writer = SegmentedWalWriter::open(dir.path(), 1 << 20).unwrap();
    for record in &records {
        writer.append(record).unwrap();
    }
    writer.sync().unwrap();
    let read: Vec<WalRecord> = read_all(dir.path())
        .unwrap()
        .into_iter()
        .map(|entry| entry.record)
        .collect();
    assert_eq!(read, records);
}

/// The commit record of earlier milestones keeps its variant index; the sequenced record is
/// appended after the catalog record.
#[test]
fn variant_indexes_are_stable() {
    let legacy = WalRecord::Commit {
        tx_id: TxId(1),
        commit_ts: CommitTs(1),
    };
    let sequenced = WalRecord::CommitV2 {
        tx_id: TxId(1),
        commit_ts: CommitTs(1),
        commit_seq: CommitSeq(1),
        schema_epoch: SchemaEpoch(0),
    };
    let legacy_bytes = bincode::serialize(&legacy).unwrap();
    let sequenced_bytes = bincode::serialize(&sequenced).unwrap();
    // Begin, Version, Put, Delete, Commit, Abort, Catalog, CommitV2.
    assert_eq!(&legacy_bytes[..4], &4u32.to_le_bytes());
    assert_eq!(&sequenced_bytes[..4], &7u32.to_le_bytes());
    assert_eq!(
        bincode::deserialize::<WalRecord>(&legacy_bytes).unwrap(),
        legacy
    );
}
