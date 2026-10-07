//! Forward cursor semantics: resuming, limits, segment rotation and truncation.

use adb_core::{CommitTs, Lsn, TxId};
use adb_wal::{earliest_lsn, read_all, SegmentedWalWriter, WalCursor, WalError, WalRecord};
use tempfile::tempdir;

fn begin(id: u64) -> WalRecord {
    WalRecord::Begin {
        tx_id: TxId(id),
        snapshot_ts: CommitTs(0),
    }
}

fn ids(entries: &[adb_wal::LogEntry]) -> Vec<u64> {
    entries
        .iter()
        .map(|entry| match entry.record {
            WalRecord::Begin { tx_id, .. } => tx_id.0,
            _ => unreachable!(),
        })
        .collect()
}

/// Small segments force many rotations; the cursor must cross them transparently and resume
/// from any `next` position it handed out.
#[test]
fn cursor_crosses_segments_and_resumes_from_any_boundary() {
    let dir = tempdir().unwrap();
    let mut writer = SegmentedWalWriter::open(dir.path(), 128).unwrap();
    for id in 0..50 {
        writer.append(&begin(id)).unwrap();
    }
    writer.sync().unwrap();
    let end = writer.position().unwrap();

    let all = read_all(dir.path()).unwrap();
    assert_eq!(ids(&all), (0..50).collect::<Vec<_>>());

    let resume_at = all[17].next;
    let mut cursor = WalCursor::open(dir.path(), resume_at).unwrap();
    let mut rest = Vec::new();
    while let Some(entry) = cursor.next_before(end).unwrap() {
        rest.push(entry);
    }
    assert_eq!(ids(&rest), (18..50).collect::<Vec<_>>());
}

/// The limit is exclusive and lets callers stop at the durable end of the log.
#[test]
fn cursor_stops_at_the_limit() {
    let dir = tempdir().unwrap();
    let mut writer = SegmentedWalWriter::open(dir.path(), 1 << 20).unwrap();
    let mut positions = Vec::new();
    for id in 0..10 {
        positions.push(writer.append(&begin(id)).unwrap());
    }
    let mut cursor = WalCursor::open(dir.path(), Lsn(0)).unwrap();
    let mut seen = Vec::new();
    while let Some(entry) = cursor.next_before(positions[4]).unwrap() {
        seen.push(entry);
    }
    assert_eq!(ids(&seen), vec![0, 1, 2, 3]);
}

/// A cursor opened before new appends still sees them (cached file length is refreshed).
#[test]
fn cursor_sees_records_appended_after_it_was_opened() {
    let dir = tempdir().unwrap();
    let mut writer = SegmentedWalWriter::open(dir.path(), 1 << 20).unwrap();
    writer.append(&begin(1)).unwrap();
    let mut cursor = WalCursor::open(dir.path(), Lsn(0)).unwrap();
    assert!(cursor.next_before(Lsn(u64::MAX)).unwrap().is_some());
    assert!(cursor.next_before(Lsn(u64::MAX)).unwrap().is_none());
    writer.append(&begin(2)).unwrap();
    assert!(cursor.next_before(Lsn(u64::MAX)).unwrap().is_some());
}

#[test]
fn positions_before_the_oldest_segment_are_reported_as_truncated() {
    let dir = tempdir().unwrap();
    {
        let mut writer = SegmentedWalWriter::open(dir.path(), 128).unwrap();
        for id in 0..20 {
            writer.append(&begin(id)).unwrap();
        }
        writer.sync().unwrap();
    }
    std::fs::remove_file(dir.path().join("0000000000000000.wal")).unwrap();
    let earliest = earliest_lsn(dir.path()).unwrap().unwrap();
    assert!(earliest > Lsn(0));
    assert!(matches!(
        WalCursor::open(dir.path(), Lsn(0)),
        Err(WalError::Truncated { .. })
    ));
}
