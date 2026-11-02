//! Waiting for projections, state changes, overtaken history and the retention frontier.
use std::{
    thread,
    time::{Duration, Instant},
};

use adb_core::{CommitSeq, ProjectionId};
use adb_projection_runtime::{
    ProjectionDescriptor, ProjectionFrontier, ProjectionKind, ProjectionMode, ProjectionRegistry,
    ProjectionState, WaitOutcome, MAX_WAIT,
};

/// Registers an asynchronous consumer `id` at `at`.
fn add(registry: &ProjectionRegistry, id: u64, at: u64, state: ProjectionState) {
    registry
        .register(
            ProjectionDescriptor::new(
                ProjectionId(id),
                format!("p{id}"),
                ProjectionKind::TestAsync,
                ProjectionMode::Asynchronous,
                None,
            ),
            state,
            ProjectionFrontier::at(CommitSeq(at)),
        )
        .unwrap();
}

/// An already satisfied wait returns at once; an unknown projection is reported.
#[test]
fn immediate_outcomes() {
    let registry = ProjectionRegistry::in_memory();
    add(&registry, 1, 5, ProjectionState::Ready);
    let started = Instant::now();
    assert_eq!(
        registry.wait_until(ProjectionId(1), CommitSeq(5), Duration::from_secs(30)),
        WaitOutcome::Reached
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        registry.wait_until(ProjectionId(9), CommitSeq(1), Duration::from_secs(30)),
        WaitOutcome::Unknown
    );
}

/// A wait that nobody satisfies times out after about the requested time.
#[test]
fn a_wait_times_out() {
    let registry = ProjectionRegistry::in_memory();
    add(&registry, 1, 5, ProjectionState::Ready);
    let started = Instant::now();
    assert_eq!(
        registry.wait_until(ProjectionId(1), CommitSeq(6), Duration::from_millis(80)),
        WaitOutcome::TimedOut
    );
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(70) && elapsed < Duration::from_secs(5));
}

/// A waiter wakes when another thread advances the frontier.
#[test]
fn a_waiter_wakes_on_advance() {
    let registry = ProjectionRegistry::in_memory();
    add(&registry, 1, 0, ProjectionState::Ready);
    let advancer = registry.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        advancer.advance(ProjectionId(1), CommitSeq(3)).unwrap();
    });
    let outcome = registry.wait_until(ProjectionId(1), CommitSeq(3), Duration::from_secs(20));
    handle.join().unwrap();
    assert_eq!(outcome, WaitOutcome::Reached);
}

/// A projection that is still being built is waited for; one that failed or needs a rebuild ends
/// the wait immediately, and so does a state change while waiting.
#[test]
fn unreadable_states() {
    let registry = ProjectionRegistry::in_memory();
    add(&registry, 1, 0, ProjectionState::Building);
    add(&registry, 2, 0, ProjectionState::Failed);
    add(&registry, 3, 0, ProjectionState::NeedsRebuild);
    assert_eq!(
        registry.wait_until(ProjectionId(2), CommitSeq(1), Duration::from_secs(20)),
        WaitOutcome::NotReadable(ProjectionState::Failed)
    );
    assert_eq!(
        registry.wait_until(ProjectionId(3), CommitSeq(1), Duration::from_secs(20)),
        WaitOutcome::NotReadable(ProjectionState::NeedsRebuild)
    );
    // Building is waited for until it becomes ready and catches up.
    let worker = registry.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        worker.advance(ProjectionId(1), CommitSeq(2)).unwrap();
        worker
            .set_state(ProjectionId(1), ProjectionState::Ready)
            .unwrap();
    });
    assert_eq!(
        registry.wait_until(ProjectionId(1), CommitSeq(2), Duration::from_secs(20)),
        WaitOutcome::Reached
    );
    handle.join().unwrap();
    // A failure while waiting wakes the waiter.
    add(&registry, 4, 0, ProjectionState::Ready);
    let breaker = registry.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        breaker
            .set_state(ProjectionId(4), ProjectionState::Failed)
            .unwrap();
    });
    assert_eq!(
        registry.wait_until(ProjectionId(4), CommitSeq(9), Duration::from_secs(20)),
        WaitOutcome::NotReadable(ProjectionState::Failed)
    );
    handle.join().unwrap();
}

/// The wait bound is capped no matter what the caller asks for.
#[test]
fn the_wait_cap_is_bounded() {
    assert!(MAX_WAIT <= Duration::from_secs(60));
}

/// History older than the change log's earliest commit overtakes slow consumers only; a rebuild
/// starts from zero and the state changes move the metadata epoch.
#[test]
fn overtaken_projections_need_a_rebuild() {
    let registry = ProjectionRegistry::in_memory();
    add(&registry, 1, 2, ProjectionState::Ready);
    add(&registry, 2, 20, ProjectionState::Ready);
    registry
        .register(
            ProjectionDescriptor::new(
                ProjectionId(3),
                "sync",
                ProjectionKind::CurrentRow,
                ProjectionMode::Synchronous,
                None,
            ),
            ProjectionState::Ready,
            ProjectionFrontier::at(CommitSeq(0)),
        )
        .unwrap();
    let epoch = registry.epoch();
    let marked = registry.mark_overtaken(CommitSeq(10));
    assert_eq!(
        marked,
        [ProjectionId(1)],
        "only the slow asynchronous consumer"
    );
    assert!(registry.epoch() > epoch);
    assert!(
        registry.mark_overtaken(CommitSeq(10)).is_empty(),
        "already marked"
    );
    assert_eq!(
        registry.get(ProjectionId(1)).unwrap().state,
        ProjectionState::NeedsRebuild
    );
    registry.begin_rebuild(ProjectionId(1)).unwrap();
    let progress = registry.get(ProjectionId(1)).unwrap();
    assert_eq!(progress.state, ProjectionState::Building);
    assert_eq!(progress.frontier.base_commit_seq, CommitSeq(0));
}

/// Retention keeps one above the slowest asynchronous reader, ignores synchronous projections
/// and projections that need a rebuild, and keeps nothing when no one reads.
#[test]
fn retention_frontier_follows_the_slowest_reader() {
    let registry = ProjectionRegistry::in_memory();
    assert_eq!(
        registry
            .retention_frontier(CommitSeq(30))
            .minimum_required_commit_seq,
        CommitSeq(31)
    );
    add(&registry, 1, 12, ProjectionState::Ready);
    add(&registry, 2, 25, ProjectionState::Ready);
    assert_eq!(
        registry
            .retention_frontier(CommitSeq(30))
            .minimum_required_commit_seq,
        CommitSeq(13)
    );
    registry
        .set_state(ProjectionId(1), ProjectionState::NeedsRebuild)
        .unwrap();
    assert_eq!(
        registry
            .retention_frontier(CommitSeq(30))
            .minimum_required_commit_seq,
        CommitSeq(26)
    );
}
