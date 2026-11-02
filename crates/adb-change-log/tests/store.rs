//! The segmented change log store: append, read back, roll over, restart and torn tails.
use adb_change_log::{
    AppendOutcome, ChangeEnvelope, ChangeLog, ChangeLogError, ChangePayload, RowChange,
};
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

/// A number below the first stored envelope (not stored, e.g. pruned) is a regression.
#[test]
fn regression_is_refused() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    log.append(&envelope(3, 4)).unwrap();
    assert!(matches!(
        log.append(&envelope(2, 4)),
        Err(ChangeLogError::Regression { head: 3, got: 2 })
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

/// Re-appending an identical envelope is a no-op (also across a restart and a segment
/// boundary); different content under the same number is a mismatch and nothing is overwritten.
#[test]
fn append_is_idempotent() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 300).unwrap();
    for seq in 1..=10 {
        assert_eq!(
            log.append(&envelope(seq, 40)).unwrap(),
            AppendOutcome::Appended
        );
    }
    for seq in [1, 5, 10] {
        assert_eq!(
            log.append(&envelope(seq, 40)).unwrap(),
            AppendOutcome::Duplicate
        );
    }
    assert_eq!(log.head(), Some(CommitSeq(10)));
    assert!(matches!(
        log.append(&envelope(5, 41)),
        Err(ChangeLogError::Mismatch { seq: 5 })
    ));
    drop(log);
    let log = ChangeLog::open(dir.path(), 300).unwrap();
    assert_eq!(
        log.append(&envelope(7, 40)).unwrap(),
        AppendOutcome::Duplicate
    );
    assert_eq!(
        seqs(&log.read_after(CommitSeq(0), 100).unwrap()),
        (1..=10).collect::<Vec<_>>()
    );
}

/// A number that is not stored (a gap below the head) cannot be inserted.
#[test]
fn gap_below_head_is_a_regression() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    log.append(&envelope(2, 4)).unwrap();
    log.append(&envelope(5, 4)).unwrap();
    assert!(matches!(
        log.append(&envelope(3, 4)),
        Err(ChangeLogError::Regression { head: 5, got: 3 })
    ));
}

/// A cursor delivers everything exactly once, in order, across batches and segments.
#[test]
fn cursor_streams_in_batches() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 300).unwrap();
    for seq in 1..=25 {
        log.append(&envelope(seq, 40)).unwrap();
    }
    let mut cursor = log.cursor(CommitSeq(0)).unwrap();
    let mut seen = Vec::new();
    while let Some(batch) = cursor.next_batch(4, u64::MAX).unwrap() {
        assert!(batch.commits.len() <= 4);
        assert_eq!(batch.first_commit_seq, batch.commits[0].commit_seq);
        assert_eq!(
            batch.last_commit_seq,
            batch.commits.last().unwrap().commit_seq
        );
        seen.extend(seqs(&batch.commits));
    }
    assert_eq!(seen, (1..=25).collect::<Vec<_>>());
    assert_eq!(cursor.position(), CommitSeq(25));
    // New commits are picked up by the same cursor.
    log.append(&envelope(26, 40)).unwrap();
    let batch = cursor.next_batch(10, u64::MAX).unwrap().unwrap();
    assert_eq!(seqs(&batch.commits), [26]);
    assert!(cursor.next_batch(10, u64::MAX).unwrap().is_none());
}

/// The byte budget limits a batch but never starves it: one oversized commit is still delivered.
#[test]
fn cursor_respects_the_byte_budget() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    for seq in 1..=6 {
        log.append(&envelope(seq, 1000)).unwrap();
    }
    let mut cursor = log.cursor(CommitSeq(0)).unwrap();
    let first = cursor.next_batch(100, 1).unwrap().unwrap();
    assert_eq!(
        first.commits.len(),
        1,
        "a tiny budget still yields one commit"
    );
    let second = cursor.next_batch(100, 2500).unwrap().unwrap();
    assert!(second.commits.len() >= 2 && second.commits.len() <= 3);
    assert!(second.bytes >= 2000);
    assert_eq!(second.first_commit_seq, CommitSeq(2));
}

/// A cursor starting inside the log skips earlier commits; one starting before the oldest stored
/// commit is refused (only an empty log accepts any position).
#[test]
fn cursor_start_positions() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
    assert!(log.cursor(CommitSeq(0)).is_ok());
    for seq in 3..=6 {
        log.append(&envelope(seq, 4)).unwrap();
    }
    let mut mid = log.cursor(CommitSeq(4)).unwrap();
    assert_eq!(
        seqs(&mid.next_batch(10, u64::MAX).unwrap().unwrap().commits),
        [5, 6]
    );
    assert!(log.cursor(CommitSeq(2)).is_ok());
    assert!(matches!(
        log.cursor(CommitSeq(1)),
        Err(ChangeLogError::Truncated {
            requested: 2,
            earliest: 3
        })
    ));
}

/// Truncating after a number drops the surplus across segments, keeps the rest readable and
/// lets the numbers be appended again with new content.
#[test]
fn truncate_after_removes_the_surplus() {
    let dir = tempdir().unwrap();
    let log = ChangeLog::open(dir.path(), 300).unwrap();
    for seq in 1..=20 {
        log.append(&envelope(seq, 40)).unwrap();
    }
    assert_eq!(log.truncate_after(CommitSeq(20)).unwrap(), 0);
    assert!(log.truncate_after(CommitSeq(8)).unwrap() > 0);
    assert_eq!(log.head(), Some(CommitSeq(8)));
    assert_eq!(
        seqs(&log.read_after(CommitSeq(0), 100).unwrap()),
        (1..=8).collect::<Vec<_>>()
    );
    // The numbers are free again, with different content.
    log.append(&envelope(9, 5)).unwrap();
    drop(log);
    let log = ChangeLog::open(dir.path(), 300).unwrap();
    assert_eq!(log.head(), Some(CommitSeq(9)));
    // Truncating below the first stored number empties the log.
    log.truncate_after(CommitSeq(0)).unwrap();
    assert_eq!(log.head(), None);
}
