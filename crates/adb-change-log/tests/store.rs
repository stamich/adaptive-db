//! The segmented change log store: append, read back, roll over, restart and torn tails.
use adb_change_log::{ChangeEnvelope, ChangeLog, ChangeLogError, ChangePayload, RowChange};
use adb_core::{CommitSeq, CommitTs, Row, RowId, SchemaEpoch, TxId, Value};
use tempfile::tempdir;

/// A data envelope with sequence number `seq` and a payload of roughly `pad` bytes.
fn envelope(seq: u64, pad: usize) -> ChangeEnvelope {
    ChangeEnvelope {
        commit_seq: CommitSeq(seq),
        commit_ts: CommitTs(seq),
        schema_epoch: SchemaEpoch(1),
        payload: ChangePayload::Data {
            tx_id: TxId(seq),
            rows: vec![RowChange {
                entity_id: 1,
                row_id: RowId(seq as u128),
                before_schema_version: None,
                after_schema_version: None,
                before: None,
                after: Some(Row::new().with_field(1, Value::String("x".repeat(pad)))),
            }],
        },
    }
}

/// Sequence numbers of a list of envelopes.
fn seqs(list: &[ChangeEnvelope]) -> Vec<u64> {
    list.iter().map(|e| e.commit_seq.0).collect()
}

/// An empty log has no head; appended envelopes are read back in order, after a cut point.
#[test]
fn append_and_read_back() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    assert_eq!(log.head(), None);
    for seq in 1..=5 {
        log.append(&envelope(seq, 4)).unwrap();
    }
    assert_eq!(log.head(), Some(CommitSeq(5)));
    assert_eq!(log.earliest(), Some(CommitSeq(1)));
    assert_eq!(
        seqs(&log.read_after(CommitSeq(0), 100).unwrap()),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(seqs(&log.read_after(CommitSeq(2), 2).unwrap()), [3, 4]);
    assert!(log.read_after(CommitSeq(5), 10).unwrap().is_empty());
}

/// A small segment size rolls the log over; everything stays readable across segments and
/// survives a restart.
#[test]
fn rollover_and_restart() {
    let dir = tempdir().unwrap();
    {
        let log = ChangeLog::open(dir.path(), 300).unwrap();
        for seq in 1..=20 {
            log.append(&envelope(seq, 40)).unwrap();
        }
        log.sync().unwrap();
        assert!(log.segment_count() > 3);
    }
    let log = ChangeLog::open(dir.path(), 300).unwrap();
    assert_eq!(log.head(), Some(CommitSeq(20)));
    assert_eq!(
        seqs(&log.read_after(CommitSeq(0), 100).unwrap()),
        (1..=20).collect::<Vec<_>>()
    );
    log.append(&envelope(21, 40)).unwrap();
    assert_eq!(log.head(), Some(CommitSeq(21)));
}

/// Sequence numbers must increase.
#[test]
fn regression_is_refused() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    log.append(&envelope(3, 4)).unwrap();
    assert!(matches!(
        log.append(&envelope(3, 4)),
        Err(ChangeLogError::Regression { head: 3, got: 3 })
    ));
    assert!(matches!(
        log.append(&envelope(2, 4)),
        Err(ChangeLogError::Regression { .. })
    ));
}

/// The path of the only segment of `dir`.
fn only_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files.pop().unwrap()
}

/// A torn last frame (every possible cut) is cut off at restart; the intact prefix survives
/// and the log accepts the next commit.
#[test]
fn torn_tail_is_truncated() {
    let dir = tempdir().unwrap();
    {
        let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
        for seq in 1..=3 {
            log.append(&envelope(seq, 10)).unwrap();
        }
    }
    let segment = only_segment(dir.path());
    let full = std::fs::read(&segment).unwrap();
    let frame_len = (full.len() - 12) / 3;
    let two = 12 + 2 * frame_len;
    for cut in two..full.len() {
        std::fs::write(&segment, &full[..cut]).unwrap();
        let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
        assert_eq!(log.head(), Some(CommitSeq(2)), "cut at {cut}");
        log.append(&envelope(3, 10)).unwrap();
        assert_eq!(seqs(&log.read_after(CommitSeq(0), 10).unwrap()), [1, 2, 3]);
    }
}

/// Damage that is followed by intact data is corruption, not a torn tail.
#[test]
fn mid_log_damage_is_an_error() {
    let dir = tempdir().unwrap();
    {
        let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
        for seq in 1..=3 {
            log.append(&envelope(seq, 10)).unwrap();
        }
    }
    let segment = only_segment(dir.path());
    let mut bytes = std::fs::read(&segment).unwrap();
    bytes[12 + 8 + 2] ^= 0xff; // inside the first frame's payload
    std::fs::write(&segment, &bytes).unwrap();
    assert!(matches!(
        ChangeLog::open(dir.path(), 1 << 20),
        Err(ChangeLogError::Corrupt { .. })
    ));
}

/// A wrong magic and an unknown format version are refused.
#[test]
fn bad_header_is_an_error() {
    let dir = tempdir().unwrap();
    {
        let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
        log.append(&envelope(1, 4)).unwrap();
    }
    let segment = only_segment(dir.path());
    let original = std::fs::read(&segment).unwrap();
    let mut magic = original.clone();
    magic[0] ^= 1;
    std::fs::write(&segment, &magic).unwrap();
    assert!(ChangeLog::open(dir.path(), 1 << 20).is_err());
    let mut version = original;
    version[8] = 9;
    std::fs::write(&segment, &version).unwrap();
    assert!(ChangeLog::open(dir.path(), 1 << 20).is_err());
}

/// An envelope above the cap is refused at append and leaves the log unchanged.
#[test]
fn oversized_envelope_is_refused() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    log.append(&envelope(1, 4)).unwrap();
    let huge = envelope(2, 65 * 1024 * 1024);
    assert!(matches!(
        log.append(&huge),
        Err(ChangeLogError::TooLarge { .. })
    ));
    assert_eq!(log.head(), Some(CommitSeq(1)));
}
