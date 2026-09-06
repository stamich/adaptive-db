//! Hardening regression tests for crash tails and bounded WAL frames.

use std::fs::OpenOptions;
use std::io::Write;

use adb_core::{CommitTs, TxId};
use adb_wal::{WalReader, WalRecord, WalWriter};
use tempfile::tempdir;

/// Verifies that reopening a writer truncates an incomplete crash tail before appending.
#[test]
fn writer_truncates_incomplete_tail_before_new_records() {
    let dir=tempdir().unwrap();
    let path=dir.path().join("wal.log");
    {
        let mut writer=WalWriter::open(&path).unwrap();
        writer.append(&WalRecord::Begin{tx_id:TxId(1),snapshot_ts:CommitTs(0)}).unwrap();
        writer.sync().unwrap();
    }
    {
        let mut file=OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0x57,0x42,0x44]).unwrap();
        file.sync_data().unwrap();
    }
    {
        let mut writer=WalWriter::open(&path).unwrap();
        writer.append(&WalRecord::Commit{tx_id:TxId(1),commit_ts:CommitTs(1)}).unwrap();
        writer.sync().unwrap();
    }
    let records=WalReader::open(&path).unwrap().read_all().unwrap();
    assert_eq!(records.len(),2);
    assert!(matches!(records[1].1,WalRecord::Commit{..}));
}

/// Verifies that an absurd length from disk is rejected before allocating its payload.
#[test]
fn reader_rejects_oversized_payload_length() {
    let dir=tempdir().unwrap();
    let path=dir.path().join("wal.log");
    let mut file=std::fs::File::create(&path).unwrap();
    file.write_all(&adb_wal::format::WAL_MAGIC.to_le_bytes()).unwrap();
    file.write_all(&adb_wal::format::WAL_VERSION.to_le_bytes()).unwrap();
    file.write_all(&u32::MAX.to_le_bytes()).unwrap();
    file.write_all(&0u32.to_le_bytes()).unwrap();
    file.sync_data().unwrap();
    assert!(WalReader::open(&path).unwrap().read_all().is_err());
}
