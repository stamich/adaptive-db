//! An asynchronous projection fed from the change log: catching up, waiting, restart, failure and
//! being overtaken by retention.
mod common;

use std::{collections::BTreeMap, thread, time::Duration};

use adb_change_log::{ChangePayload, ChangeRetentionPolicy};
use adb_core::{CommitSeq, RowId};
use adb_engine::{Database, DatabaseOptions, DbError};
use adb_projection_runtime::{ProjectionState, WaitOutcome};
use common::{delete, put};
use tempfile::tempdir;

/// A toy projection: the latest value of every row, applied from change envelopes.
#[derive(Default)]
struct Mirror {
    /// Row id to the number of changes applied to it.
    rows: BTreeMap<u128, u32>,
    /// Commits applied, in order.
    applied: Vec<u64>,
}

impl Mirror {
    /// Applies one batch.
    fn apply(&mut self, batch: &adb_change_log::ChangeBatch) -> Result<(), DbError> {
        for commit in &batch.commits {
            self.applied.push(commit.commit_seq.0);
            if let ChangePayload::Data { rows, .. } = &commit.payload {
                for row in rows {
                    *self.rows.entry(row.row_id.0).or_insert(0) += 1;
                    if row.after.is_none() {
                        self.rows.remove(&row.row_id.0);
                    }
                }
            }
        }
        Ok(())
    }
}

/// The projection catches up in batches, becomes ready and then follows new commits.
#[test]
fn a_consumer_catches_up_and_follows() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for i in 1..=7 {
        put(&db, RowId(i), i as i64);
    }
    delete(&db, RowId(3));
    let id = db.register_consumer_projection("mirror", None).unwrap();
    assert_eq!(db.register_consumer_projection("mirror", None).unwrap(), id);
    let registry = db.projection_registry();
    assert_eq!(registry.get(id).unwrap().state, ProjectionState::Building);
    assert_eq!(registry.get(id).unwrap().lag_commits, 8);

    let mut mirror = Mirror::default();
    let applied = db
        .feed_projection(id, (3, u64::MAX), |b| mirror.apply(b))
        .unwrap();
    assert_eq!(applied, 8);
    assert_eq!(mirror.applied, (1..=8).collect::<Vec<_>>());
    assert_eq!(mirror.rows.len(), 6);
    let progress = registry.get(id).unwrap();
    assert_eq!(progress.state, ProjectionState::Ready);
    assert_eq!(progress.lag_commits, 0);
    assert!(progress.commits_per_second >= 0.0);

    put(&db, RowId(9), 9);
    assert_eq!(registry.get(id).unwrap().lag_commits, 1);
    assert_eq!(
        db.feed_projection(id, (10, u64::MAX), |b| mirror.apply(b))
            .unwrap(),
        1
    );
    assert_eq!(
        db.feed_projection(id, (10, u64::MAX), |b| mirror.apply(b))
            .unwrap(),
        0
    );
    assert_eq!(mirror.applied.last(), Some(&9));
}

/// A reader can wait for the projection while another thread feeds it.
#[test]
fn a_reader_waits_for_the_consumer() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let id = db.register_consumer_projection("mirror", None).unwrap();
    let mut mirror = Mirror::default();
    db.feed_projection(id, (10, u64::MAX), |b| mirror.apply(b))
        .unwrap();
    put(&db, RowId(1), 1);
    let target = db.latest_commit_seq();
    let feeder = db.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(60));
        let mut mirror = Mirror::default();
        feeder
            .feed_projection(id, (10, u64::MAX), |b| mirror.apply(b))
            .unwrap();
    });
    let outcome = db
        .projection_registry()
        .wait_until(id, target, Duration::from_secs(20));
    handle.join().unwrap();
    assert_eq!(outcome, WaitOutcome::Reached);
}

/// The frontier saved by a checkpoint is where the projection resumes after a restart.
#[test]
fn a_restart_resumes_after_the_saved_frontier() {
    let dir = tempdir().unwrap();
    let id;
    {
        let db = Database::open(dir.path()).unwrap();
        for i in 1..=6 {
            put(&db, RowId(i), i as i64);
        }
        id = db.register_consumer_projection("mirror", None).unwrap();
        let mut mirror = Mirror::default();
        db.feed_projection(id, (4, u64::MAX), |b| mirror.apply(b))
            .unwrap();
        db.close().unwrap();
        put(&db, RowId(7), 7);
    }
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.register_consumer_projection("mirror", None).unwrap(), id);
    let progress = db.projection_registry().get(id).unwrap();
    assert_eq!(progress.frontier.base_commit_seq, CommitSeq(6));
    assert_eq!(progress.lag_commits, 1);
    let mut mirror = Mirror::default();
    db.feed_projection(id, (10, u64::MAX), |b| mirror.apply(b))
        .unwrap();
    assert_eq!(mirror.applied, [7]);
}

/// A failing consumer is marked failed and keeps its frontier; it is not fed again until reset.
#[test]
fn a_failing_apply_marks_the_projection_failed() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for i in 1..=3 {
        put(&db, RowId(i), i as i64);
    }
    let id = db.register_consumer_projection("mirror", None).unwrap();
    let result = db.feed_projection(id, (10, u64::MAX), |_| {
        Err(DbError::InvalidArgument("boom".into()))
    });
    assert!(result.is_err());
    let progress = db.projection_registry().get(id).unwrap();
    assert_eq!(progress.state, ProjectionState::Failed);
    assert_eq!(progress.frontier.base_commit_seq, CommitSeq::ZERO);
    assert!(db.feed_projection(id, (10, u64::MAX), |_| Ok(())).is_err());
    // The owner can rebuild it from scratch.
    db.projection_registry().begin_rebuild(id).unwrap();
    let mut mirror = Mirror::default();
    assert_eq!(
        db.feed_projection(id, (10, u64::MAX), |b| mirror.apply(b))
            .unwrap(),
        3
    );
}

/// Options with a tiny change-log segment, so a few commits span many segments.
fn small_segments() -> DatabaseOptions {
    DatabaseOptions {
        change_log_segment_bytes: 400,
        ..DatabaseOptions::default()
    }
}

/// A slow consumer holds the change log back; a fast one lets it go; once a hard limit removes
/// what the slow one needs it is marked for rebuild and the synchronous projections are never
/// affected.
#[test]
fn retention_waits_for_consumers_until_a_limit_overtakes_them() {
    let dir = tempdir().unwrap();
    let db = Database::open_with(dir.path(), small_segments()).unwrap();
    for i in 1..=40 {
        put(&db, RowId(i), i as i64);
    }
    let slow = db.register_consumer_projection("slow", None).unwrap();
    // Nothing consumed yet: the frontier alone removes nothing.
    let report = db
        .prune_change_log(&ChangeRetentionPolicy::default())
        .unwrap();
    assert_eq!(report.segments_removed, 0);
    assert_eq!(db.change_log().earliest(), Some(CommitSeq(1)));

    // The consumer reads the whole history; everything before the active segment can go.
    let mut mirror = Mirror::default();
    db.feed_projection(slow, (20, u64::MAX), |b| mirror.apply(b))
        .unwrap();
    assert_eq!(mirror.applied.len(), 40);
    let report = db
        .prune_change_log(&ChangeRetentionPolicy::default())
        .unwrap();
    assert!(report.segments_removed > 0 && !report.overtook_frontier);
    assert!(db.change_log().earliest().unwrap() > CommitSeq(1));
    assert_eq!(
        db.projection_registry().get(slow).unwrap().state,
        ProjectionState::Ready
    );

    // A second consumer that never read anything is overtaken by the lag limit.
    let late = db.register_consumer_projection("late", None).unwrap();
    for i in 41..=80 {
        put(&db, RowId(i), i as i64);
    }
    let limit = ChangeRetentionPolicy {
        max_lag_commits: Some(10),
        ..Default::default()
    };
    let report = db.prune_change_log(&limit).unwrap();
    assert!(report.overtook_frontier);
    let progress = db.projection_registry().get(late).unwrap();
    assert_eq!(progress.state, ProjectionState::NeedsRebuild);
    assert!(db
        .feed_projection(late, (10, u64::MAX), |_| Ok(()))
        .is_err());
    // The synchronous projections do not read the change log.
    let current = db
        .projection_registry()
        .get(adb_engine::projection_runtime::CURRENT_ROW_PROJECTION)
        .unwrap();
    assert_eq!(current.state, ProjectionState::Ready);
}
