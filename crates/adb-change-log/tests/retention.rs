//! Retention: consumed segments go, limits bound the log, the active segment stays.
use adb_change_log::{
    ChangeEnvelope, ChangeLog, ChangeLogError, ChangePayload, ChangeRetentionPolicy,
    RetentionFrontier,
};
use adb_core::{CommitSeq, CommitTs, SchemaEpoch, TxId};
use tempfile::tempdir;

/// A small envelope with sequence number `seq`.
fn envelope(seq: u64) -> ChangeEnvelope {
    ChangeEnvelope {
        commit_seq: CommitSeq(seq),
        commit_ts: CommitTs(seq),
        schema_epoch: SchemaEpoch(1),
        payload: ChangePayload::Data {
            tx_id: TxId(seq),
            rows: Vec::new(),
        },
    }
}

/// A log of 30 commits with about three commits per segment.
fn filled(dir: &std::path::Path) -> ChangeLog {
    let log = ChangeLog::open(dir, 120).unwrap();
    for seq in 1..=30 {
        log.append(&envelope(seq)).unwrap();
    }
    log
}

/// The frontier at `seq`.
fn frontier(seq: u64) -> RetentionFrontier {
    RetentionFrontier {
        minimum_required_commit_seq: CommitSeq(seq),
    }
}

/// Only whole segments below the frontier are removed; the commit the frontier names stays
/// readable and a cursor positioned just before it keeps working.
#[test]
fn consumed_segments_are_removed() {
    let dir = tempdir().unwrap();
    let log = filled(dir.path());
    let before = log.segment_count();
    let report = log
        .prune(frontier(12), &ChangeRetentionPolicy::default())
        .unwrap();
    assert!(report.segments_removed > 0 && report.segments_removed < before);
    assert!(!report.overtook_frontier);
    assert_eq!(log.segment_count(), before - report.segments_removed);
    let earliest = log.earliest().unwrap();
    assert!(
        earliest <= CommitSeq(12),
        "the commit the frontier names is kept"
    );
    let mut cursor = log.cursor(CommitSeq(11)).unwrap();
    let batch = cursor.next_batch(1, u64::MAX).unwrap().unwrap();
    assert_eq!(batch.first_commit_seq, CommitSeq(12));
    assert_eq!(log.head(), Some(CommitSeq(30)));
}

/// Even a frontier beyond the head keeps the active segment.
#[test]
fn the_active_segment_is_never_removed() {
    let dir = tempdir().unwrap();
    let log = filled(dir.path());
    log.prune(frontier(1_000), &ChangeRetentionPolicy::default())
        .unwrap();
    assert_eq!(log.segment_count(), 1);
    assert_eq!(log.head(), Some(CommitSeq(30)));
    log.append(&envelope(31)).unwrap();
}

/// A size limit removes unconsumed history, reports that it overtook the frontier and the
/// affected consumer sees `Truncated`.
#[test]
fn limits_bound_the_log_even_against_a_slow_consumer() {
    let dir = tempdir().unwrap();
    let log = filled(dir.path());
    let full = log.total_bytes();
    let policy = ChangeRetentionPolicy {
        max_bytes: Some(full / 3),
        ..Default::default()
    };
    let report = log.prune(frontier(1), &policy).unwrap();
    assert!(report.overtook_frontier);
    assert!(log.total_bytes() <= full / 3 + 200);
    assert!(matches!(
        log.cursor(CommitSeq(0)),
        Err(ChangeLogError::Truncated { requested: 1, .. })
    ));
}

/// The lag limit counts commits between the head and the start of a segment.
#[test]
fn lag_limit() {
    let dir = tempdir().unwrap();
    let log = filled(dir.path());
    let policy = ChangeRetentionPolicy {
        max_lag_commits: Some(10),
        ..Default::default()
    };
    let report = log.prune(frontier(1), &policy).unwrap();
    assert!(report.overtook_frontier);
    assert!(30 - log.earliest().unwrap().0 <= 10);
}

/// A limit that is not exceeded removes nothing, and pruning survives a restart.
#[test]
fn generous_limits_keep_everything_and_restart_works() {
    let dir = tempdir().unwrap();
    {
        let log = filled(dir.path());
        let policy = ChangeRetentionPolicy {
            max_bytes: Some(u64::MAX),
            max_age_seconds: Some(3600),
            max_lag_commits: Some(1_000),
        };
        let report = log.prune(frontier(1), &policy).unwrap();
        assert_eq!(report.segments_removed, 0);
        log.prune(frontier(20), &ChangeRetentionPolicy::default())
            .unwrap();
    }
    let log = ChangeLog::open(dir.path(), 120).unwrap();
    assert!(log.earliest().unwrap() > CommitSeq(1));
    assert_eq!(log.head(), Some(CommitSeq(30)));
}
