//! Regression tests for segmented-WAL crash-tail and corruption hardening.

use std::{
    fs::{self, OpenOptions},
    io::Write,
};

use adb_core::{CommitTs, TxId};
use adb_wal::{read_all, SegmentedWalWriter, WalRecord};
use tempfile::tempdir;

/// Verifies that reopening the final WAL segment truncates only an incomplete
/// crash tail before new records are appended.
#[test]
fn writer_truncates_incomplete_tail_before_append() {
    let dir = tempdir().unwrap();
    let wal_dir = dir.path().join("wal");

    {
        let mut writer = SegmentedWalWriter::open(&wal_dir, 1024 * 1024).unwrap();
        writer
            .append(&WalRecord::Begin {
                tx_id: TxId(1),
                snapshot_ts: CommitTs(0),
            })
            .unwrap();
        writer.sync().unwrap();
    }

    let mut segments: Vec<_> = fs::read_dir(&wal_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    segments.sort();
    let last = segments.last().unwrap();
    {
        let mut file = OpenOptions::new().append(true).open(last).unwrap();
        file.write_all(&[0x57, 0x42, 0x44]).unwrap();
        file.sync_data().unwrap();
    }

    {
        let mut writer = SegmentedWalWriter::open(&wal_dir, 1024 * 1024).unwrap();
        writer
            .append(&WalRecord::Commit {
                tx_id: TxId(1),
                commit_ts: CommitTs(1),
            })
            .unwrap();
        writer.sync().unwrap();
    }

    let records = read_all(&wal_dir).unwrap();
    assert_eq!(records.len(), 2);
    assert!(matches!(records[1].record, WalRecord::Commit { .. }));
}
