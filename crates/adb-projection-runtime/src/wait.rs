//! Waiting for a projection, state changes and the retention frontier.
//!
//! These operations complete the registry: a reader that needs a projection to reach a commit
//! can block (bounded) until it does, a projection whose history was pruned is marked for
//! rebuild, and the change log's retention asks which commits must still be kept.
use std::time::{Duration, Instant};

use adb_change_log::RetentionFrontier;
use adb_core::{CommitSeq, ProjectionId};

use crate::{
    error::ProjectionError,
    model::{ProjectionMode, ProjectionState},
    registry::{bump, unknown, ProjectionRegistry},
};

/// Longest a single wait may last, whatever the caller asks for. A bound that always applies
/// keeps a session from parking a thread forever on a stalled projection.
pub const MAX_WAIT: Duration = Duration::from_secs(60);

/// How a [`ProjectionRegistry::wait_until`] call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// The projection is ready and covers the requested commit.
    Reached,
    /// The time limit elapsed first.
    TimedOut,
    /// The projection cannot reach the commit by itself in its current state (it needs a
    /// rebuild or failed); waiting longer would not help.
    NotReadable(ProjectionState),
    /// The projection is not (or no longer) registered.
    Unknown,
}

impl ProjectionRegistry {
    /// Changes the state of `id`. A change of state moves the metadata epoch and wakes waiters.
    pub fn set_state(
        &self,
        id: ProjectionId,
        state: ProjectionState,
    ) -> Result<(), ProjectionError> {
        let mut table = self.shared.table.lock();
        let entry = table
            .entries
            .iter_mut()
            .find(|e| e.descriptor.id == id)
            .ok_or_else(|| unknown(id))?;
        if entry.state != state {
            entry.state = state;
            bump(&mut table);
        }
        self.shared.changed.notify_all();
        Ok(())
    }

    /// Starts rebuilding `id` from scratch: state [`ProjectionState::Building`], frontier zero.
    pub fn begin_rebuild(&self, id: ProjectionId) -> Result<(), ProjectionError> {
        let mut table = self.shared.table.lock();
        let entry = table
            .entries
            .iter_mut()
            .find(|e| e.descriptor.id == id)
            .ok_or_else(|| unknown(id))?;
        entry.state = ProjectionState::Building;
        entry.frontier = crate::model::ProjectionFrontier::at(CommitSeq::ZERO);
        entry.lag_changes = 0;
        entry.lag_bytes = 0;
        bump(&mut table);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// Blocks until `id` is ready and has consumed commit `seq`, for at most `timeout`
    /// (capped at [`MAX_WAIT`]).
    ///
    /// A projection that is being built keeps the waiter waiting; one that needs a rebuild or has
    /// failed ends the wait at once with [`WaitOutcome::NotReadable`], because only an outside
    /// action can change that.
    pub fn wait_until(&self, id: ProjectionId, seq: CommitSeq, timeout: Duration) -> WaitOutcome {
        let deadline = Instant::now() + timeout.min(MAX_WAIT);
        let mut table = self.shared.table.lock();
        loop {
            let Some(entry) = table.entries.iter().find(|e| e.descriptor.id == id) else {
                return WaitOutcome::Unknown;
            };
            match entry.state {
                ProjectionState::Ready if entry.frontier.readable_through() >= seq => {
                    return WaitOutcome::Reached;
                }
                ProjectionState::NeedsRebuild | ProjectionState::Failed => {
                    return WaitOutcome::NotReadable(entry.state);
                }
                ProjectionState::Ready | ProjectionState::Building => {}
            }
            if self
                .shared
                .changed
                .wait_until(&mut table, deadline)
                .timed_out()
            {
                // One last look: the change may have raced the timeout.
                let reached = table.entries.iter().any(|e| {
                    e.descriptor.id == id
                        && e.state == ProjectionState::Ready
                        && e.frontier.readable_through() >= seq
                });
                return if reached {
                    WaitOutcome::Reached
                } else {
                    WaitOutcome::TimedOut
                };
            }
        }
    }

    /// Moves every ready synchronous projection to `to`. The commit path calls this once per
    /// commit, after the projections were applied: a synchronous projection is by construction
    /// as far as the commit that was just applied.
    pub fn advance_synchronous(&self, to: CommitSeq) {
        let mut table = self.shared.table.lock();
        for entry in table.entries.iter_mut() {
            if entry.descriptor.mode == ProjectionMode::Synchronous
                && entry.state == ProjectionState::Ready
                && to > entry.frontier.base_commit_seq
            {
                entry.frontier.base_commit_seq = to;
                entry.frontier.delta_commit_seq = entry.frontier.delta_commit_seq.max(to);
            }
        }
        self.shared.changed.notify_all();
    }

    /// Marks asynchronous projections that need history older than `earliest` (the oldest
    /// commit still in the change log) as [`ProjectionState::NeedsRebuild`]. Returns their ids.
    pub fn mark_overtaken(&self, earliest: CommitSeq) -> Vec<ProjectionId> {
        let mut table = self.shared.table.lock();
        let mut marked = Vec::new();
        for entry in table.entries.iter_mut() {
            let needs = entry.frontier.base_commit_seq.0 + 1;
            if entry.descriptor.mode == ProjectionMode::Asynchronous
                && matches!(
                    entry.state,
                    ProjectionState::Building | ProjectionState::Ready
                )
                && needs < earliest.0
            {
                entry.state = ProjectionState::NeedsRebuild;
                marked.push(entry.descriptor.id);
            }
        }
        if !marked.is_empty() {
            bump(&mut table);
            self.shared.changed.notify_all();
        }
        marked
    }

    /// The lowest commit the change log must keep for the projections: one above the smallest
    /// frontier among those that still read it (asynchronous, not waiting for a rebuild).
    /// With none, nothing up to `head` is needed.
    pub fn retention_frontier(&self, head: CommitSeq) -> RetentionFrontier {
        let table = self.shared.table.lock();
        let lowest = table
            .entries
            .iter()
            .filter(|e| {
                e.descriptor.mode == ProjectionMode::Asynchronous
                    && e.state != ProjectionState::NeedsRebuild
            })
            .map(|e| e.frontier.base_commit_seq.0 + 1)
            .min();
        RetentionFrontier {
            minimum_required_commit_seq: CommitSeq(lowest.unwrap_or(head.0 + 1)),
        }
    }
}
