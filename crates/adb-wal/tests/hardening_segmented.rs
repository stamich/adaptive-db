//! Regression tests for segmented WAL hardening.
use adb_core::{CommitTs, TxId};
use adb_wal::{SegmentedWalReader, SegmentedWalWriter, WalRecord};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};
use tempfile::tempdir;
/// Verifies that an incomplete suffix of the newest segment is truncated before later appends.
#[test]
fn segmented_writer_truncates_crash_tail() {
    let d = tempdir().unwrap();
    {
        let mut w = SegmentedWalWriter::open(d.path(), 1024 * 1024).unwrap();
        w.append(&WalRecord::Begin {
            tx_id: TxId(1),
            snapshot_ts: CommitTs(0),
        })
            .unwrap();
        w.sync().unwrap();
    }
    let p = fs::read_dir(d.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    {
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&[0x57, 0x42]).unwrap();
        f.sync_data().unwrap();
    }
    {
        let mut w = SegmentedWalWriter::open(d.path(), 1024 * 1024).unwrap();
        w.append(&WalRecord::Commit {
            tx_id: TxId(1),
            commit_ts: CommitTs(1),
        })
            .unwrap();
        w.sync().unwrap();
    }
    assert_eq!(SegmentedWalReader::read_all(d.path()).unwrap().len(), 2);
}
/// Verifies that a gap between retained WAL segment ids is detected as corruption.
#[test]
fn segment_gap_is_rejected() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("0000000000000001.wal"), []).unwrap();
    fs::write(d.path().join("0000000000000003.wal"), []).unwrap();
    assert!(SegmentedWalReader::read_all(d.path()).is_err());
}
/// Verifies invalid segment sizes fail with an error instead of triggering packed-LSN assertions.
#[test]
fn invalid_segment_size_is_rejected() {
    let d = tempdir().unwrap();
    assert!(SegmentedWalWriter::open(d.path(), 1).is_err());
}
