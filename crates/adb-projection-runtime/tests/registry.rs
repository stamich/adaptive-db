//! The projection registry: registration, monotonic frontiers, lag, rate and persistence.
use adb_core::{CommitSeq, ProjectionEpoch, ProjectionId};
use adb_projection_runtime::{
    ProjectionDescriptor, ProjectionFrontier, ProjectionKind, ProjectionMode, ProjectionRegistry,
    ProjectionState,
};
use tempfile::tempdir;

/// An asynchronous consumer descriptor.
fn consumer(id: u64, name: &str) -> ProjectionDescriptor {
    ProjectionDescriptor::new(
        ProjectionId(id),
        name,
        ProjectionKind::TestAsync,
        ProjectionMode::Asynchronous,
        Some(1),
    )
}

/// Registering is idempotent for the same definition and refuses a conflicting one.
#[test]
fn register_get_list_remove() {
    let registry = ProjectionRegistry::in_memory();
    let start = registry.epoch();
    registry
        .register(
            consumer(2, "b"),
            ProjectionState::Building,
            ProjectionFrontier::at(CommitSeq(0)),
        )
        .unwrap();
    registry
        .register(
            consumer(1, "a"),
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(4)),
        )
        .unwrap();
    assert!(registry.epoch() > start);
    let epoch = registry.epoch();
    // The same definition again changes nothing, not even the frontier.
    registry
        .register(
            consumer(1, "a"),
            ProjectionState::Building,
            ProjectionFrontier::at(CommitSeq(0)),
        )
        .unwrap();
    assert_eq!(registry.epoch(), epoch);
    assert_eq!(
        registry
            .get(ProjectionId(1))
            .unwrap()
            .frontier
            .base_commit_seq,
        CommitSeq(4)
    );
    // Same name under another id, same id with another kind: both conflict.
    assert!(registry
        .register(
            consumer(9, "a"),
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(0))
        )
        .is_err());
    let mut other = consumer(1, "a");
    other.entity_id = Some(77);
    assert!(registry
        .register(
            other,
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(0))
        )
        .is_err());
    let ids: Vec<u64> = registry.list().iter().map(|p| p.descriptor.id.0).collect();
    assert_eq!(ids, [1, 2], "listed by id");
    assert_eq!(registry.find("b").unwrap().descriptor.id, ProjectionId(2));
    assert!(registry.remove(ProjectionId(2)));
    assert!(!registry.remove(ProjectionId(2)));
    assert_eq!(registry.list().len(), 1);
}

/// Frontiers move forward only; lag is measured against the head, which never moves back.
#[test]
fn frontiers_are_monotonic_and_lag_follows_the_head() {
    let registry = ProjectionRegistry::in_memory();
    registry
        .register(
            consumer(1, "a"),
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(0)),
        )
        .unwrap();
    registry.set_head(CommitSeq(10));
    registry.set_head(CommitSeq(8));
    registry.advance(ProjectionId(1), CommitSeq(6)).unwrap();
    registry.advance(ProjectionId(1), CommitSeq(6)).unwrap();
    assert!(registry.advance(ProjectionId(1), CommitSeq(5)).is_err());
    assert!(registry.advance(ProjectionId(77), CommitSeq(5)).is_err());
    let progress = registry.get(ProjectionId(1)).unwrap();
    assert_eq!(progress.head_commit_seq, CommitSeq(10));
    assert_eq!(progress.lag_commits, 4);
    assert!(progress.covers(CommitSeq(6)) && !progress.covers(CommitSeq(7)));
    registry.report_lag(ProjectionId(1), 12, 3400).unwrap();
    let progress = registry.get(ProjectionId(1)).unwrap();
    assert_eq!((progress.lag_changes, progress.lag_bytes), (12, 3400));
    // Advancing past the head is allowed (a synchronous projection moves before publication).
    registry.advance(ProjectionId(1), CommitSeq(11)).unwrap();
    assert_eq!(registry.get(ProjectionId(1)).unwrap().lag_commits, 0);
}

/// The rate is a smoothed average and refuses non-finite or negative observations.
#[test]
fn rate_is_smoothed_and_validated() {
    let registry = ProjectionRegistry::in_memory();
    registry
        .register(
            consumer(1, "a"),
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(0)),
        )
        .unwrap();
    registry.observe_rate(ProjectionId(1), 100.0).unwrap();
    assert_eq!(
        registry.get(ProjectionId(1)).unwrap().commits_per_second,
        100.0
    );
    registry.observe_rate(ProjectionId(1), 200.0).unwrap();
    let smoothed = registry.get(ProjectionId(1)).unwrap().commits_per_second;
    assert!(smoothed > 100.0 && smoothed < 200.0);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(registry.observe_rate(ProjectionId(1), bad).is_err());
    }
    assert_eq!(
        registry.get(ProjectionId(1)).unwrap().commits_per_second,
        smoothed
    );
}

/// Replacing a definition restarts the projection and moves its definition epoch.
#[test]
fn redefine_restarts_the_projection() {
    let registry = ProjectionRegistry::in_memory();
    registry
        .register(
            consumer(1, "a"),
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(9)),
        )
        .unwrap();
    let mut changed = consumer(1, "a");
    changed.entity_id = Some(5);
    let epoch = registry.epoch();
    registry.redefine(changed).unwrap();
    let progress = registry.get(ProjectionId(1)).unwrap();
    assert_eq!(progress.descriptor.epoch, ProjectionEpoch(2));
    assert_eq!(progress.state, ProjectionState::Building);
    assert_eq!(progress.frontier.base_commit_seq, CommitSeq(0));
    assert!(registry.epoch() > epoch);
    assert!(registry.redefine(consumer(42, "zzz")).is_err());
}

/// A checkpointed registry comes back with its projections, states, frontiers and epoch.
#[test]
fn checkpoint_and_restart() {
    let dir = tempdir().unwrap();
    {
        let registry = ProjectionRegistry::open(dir.path()).unwrap();
        registry
            .register(
                consumer(1, "a"),
                ProjectionState::Ready,
                ProjectionFrontier::at(CommitSeq(0)),
            )
            .unwrap();
        registry.advance(ProjectionId(1), CommitSeq(33)).unwrap();
        registry.checkpoint().unwrap();
        // Progress after the checkpoint is lost on a crash, by design.
        registry.advance(ProjectionId(1), CommitSeq(40)).unwrap();
    }
    let registry = ProjectionRegistry::open(dir.path()).unwrap();
    let progress = registry.get(ProjectionId(1)).unwrap();
    assert_eq!(progress.frontier.base_commit_seq, CommitSeq(33));
    assert_eq!(progress.state, ProjectionState::Ready);
    assert!(registry.epoch() > ProjectionEpoch(0));
}
