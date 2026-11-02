//! Descriptors, states, frontiers and progress of projections.
use adb_core::{CommitSeq, ProjectionEpoch, ProjectionId};
use serde::{Deserialize, Serialize};

use crate::error::ProjectionError;

/// Longest accepted projection name in bytes.
pub const MAX_PROJECTION_NAME_BYTES: usize = 128;

/// What a projection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectionKind {
    /// The store of current rows that serves point lookups and scans.
    CurrentRow,
    /// A secondary index over one entity.
    SecondaryIndex,
    /// A consumer used by tests and the demo: it follows the change log at its own pace.
    TestAsync,
}

/// How a projection is kept up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectionMode {
    /// Updated inside the commit; its frontier equals the published head.
    Synchronous,
    /// Updated by a consumer of the change log; its frontier may trail the head.
    Asynchronous,
}

/// Lifecycle state of a projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectionState {
    /// Being built; not readable yet.
    Building,
    /// Readable up to its frontier.
    Ready,
    /// The change log no longer holds what the projection needs: it must be rebuilt.
    NeedsRebuild,
    /// The projection stopped after an error and is not readable.
    Failed,
}

impl ProjectionState {
    /// Whether reads may be served from a projection in this state.
    pub fn is_readable(self) -> bool {
        self == ProjectionState::Ready
    }
}

/// How far a projection has consumed the commit order.
///
/// `base_commit_seq` is the commit the projection's own structure covers. `delta_commit_seq` is
/// reserved for an overlay that would bring reads further than the base (delta repair, planned
/// after 2.5.0); in 2.5.0 it always equals the base, and a projection is readable through
/// [`ProjectionFrontier::readable_through`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectionFrontier {
    /// Last commit covered by the projection's own structure.
    pub base_commit_seq: CommitSeq,
    /// Last commit covered when the delta overlay is included; never below the base.
    pub delta_commit_seq: CommitSeq,
}

impl ProjectionFrontier {
    /// A frontier at `seq` with no delta overlay.
    pub fn at(seq: CommitSeq) -> Self {
        Self {
            base_commit_seq: seq,
            delta_commit_seq: seq,
        }
    }

    /// The last commit a read from this projection reflects.
    pub fn readable_through(&self) -> CommitSeq {
        self.delta_commit_seq.max(self.base_commit_seq)
    }

    /// Whether the frontier is consistent (the delta never trails the base).
    pub fn is_valid(&self) -> bool {
        self.delta_commit_seq >= self.base_commit_seq
    }
}

/// Static description of a projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionDescriptor {
    /// Stable identifier.
    pub id: ProjectionId,
    /// Unique, human-readable name (`__current_row`, an index name, ...).
    pub name: String,
    /// What the projection is.
    pub kind: ProjectionKind,
    /// How it is maintained.
    pub mode: ProjectionMode,
    /// Entity the projection covers; `None` for a projection of the whole database.
    pub entity_id: Option<u64>,
    /// Version of the projection's definition; changes when the definition is replaced, so a
    /// session can tell that a plan made against the old definition is stale.
    pub epoch: ProjectionEpoch,
    /// Whether reads may add a delta overlay on top of the base (always `false` in 2.5.0).
    pub supports_delta_repair: bool,
}

impl ProjectionDescriptor {
    /// A descriptor with epoch 1 and no delta repair.
    pub fn new(
        id: ProjectionId,
        name: impl Into<String>,
        kind: ProjectionKind,
        mode: ProjectionMode,
        entity_id: Option<u64>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            mode,
            entity_id,
            epoch: ProjectionEpoch(1),
            supports_delta_repair: false,
        }
    }

    /// Checks the name and the capabilities the runtime supports.
    pub fn validate(&self) -> Result<(), ProjectionError> {
        if self.name.is_empty() || self.name.len() > MAX_PROJECTION_NAME_BYTES {
            return Err(ProjectionError::Invalid(format!(
                "projection name must be 1..={MAX_PROJECTION_NAME_BYTES} bytes"
            )));
        }
        if self.name.chars().any(char::is_control) {
            return Err(ProjectionError::Invalid(
                "projection name must not contain control characters".into(),
            ));
        }
        if self.supports_delta_repair {
            return Err(ProjectionError::Invalid(
                "delta repair is not available in this version".into(),
            ));
        }
        if self.kind == ProjectionKind::TestAsync && self.mode == ProjectionMode::Synchronous {
            return Err(ProjectionError::Invalid(
                "a change-log consumer is asynchronous by definition".into(),
            ));
        }
        Ok(())
    }
}

/// A projection together with its current position and lag, as reported to readers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionProgress {
    /// The projection.
    pub descriptor: ProjectionDescriptor,
    /// Lifecycle state.
    pub state: ProjectionState,
    /// How far it has consumed the commit order.
    pub frontier: ProjectionFrontier,
    /// Head of the commit order when the progress was taken.
    pub head_commit_seq: CommitSeq,
    /// Commits between the frontier and the head.
    pub lag_commits: u64,
    /// Row/schema changes between the frontier and the head, when the consumer reports them.
    pub lag_changes: u64,
    /// Encoded change-log bytes between the frontier and the head, when reported.
    pub lag_bytes: u64,
    /// Smoothed consumption speed in commits per second; 0 until observed.
    pub commits_per_second: f64,
}

impl ProjectionProgress {
    /// Whether the projection reflects every commit up to `seq`.
    pub fn covers(&self, seq: CommitSeq) -> bool {
        self.state.is_readable() && self.frontier.readable_through() >= seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed descriptor passes; names are bounded and free of control characters.
    #[test]
    fn descriptor_validation() {
        let ok = ProjectionDescriptor::new(
            ProjectionId(1),
            "__current_row",
            ProjectionKind::CurrentRow,
            ProjectionMode::Synchronous,
            None,
        );
        assert!(ok.validate().is_ok());
        let mut bad = ok.clone();
        bad.name = String::new();
        assert!(bad.validate().is_err());
        bad.name = "x".repeat(MAX_PROJECTION_NAME_BYTES + 1);
        assert!(bad.validate().is_err());
        bad.name = "a\nb".into();
        assert!(bad.validate().is_err());
        let mut delta = ok.clone();
        delta.supports_delta_repair = true;
        assert!(delta.validate().is_err());
        let sync_consumer = ProjectionDescriptor::new(
            ProjectionId(2),
            "c",
            ProjectionKind::TestAsync,
            ProjectionMode::Synchronous,
            None,
        );
        assert!(sync_consumer.validate().is_err());
    }

    /// A frontier reads through the larger of its two positions and flags an inverted pair.
    #[test]
    fn frontier_semantics() {
        let at = ProjectionFrontier::at(CommitSeq(5));
        assert_eq!(at.readable_through(), CommitSeq(5));
        assert!(at.is_valid());
        let inverted = ProjectionFrontier {
            base_commit_seq: CommitSeq(5),
            delta_commit_seq: CommitSeq(3),
        };
        assert!(!inverted.is_valid());
        assert_eq!(inverted.readable_through(), CommitSeq(5));
    }

    /// Only a ready projection covers a sequence number, and only up to its frontier.
    #[test]
    fn coverage_requires_readiness() {
        let descriptor = ProjectionDescriptor::new(
            ProjectionId(3),
            "p",
            ProjectionKind::TestAsync,
            ProjectionMode::Asynchronous,
            Some(1),
        );
        let mut progress = ProjectionProgress {
            descriptor,
            state: ProjectionState::Ready,
            frontier: ProjectionFrontier::at(CommitSeq(10)),
            head_commit_seq: CommitSeq(12),
            lag_commits: 2,
            lag_changes: 0,
            lag_bytes: 0,
            commits_per_second: 0.0,
        };
        assert!(progress.covers(CommitSeq(10)));
        assert!(!progress.covers(CommitSeq(11)));
        progress.state = ProjectionState::NeedsRebuild;
        assert!(!progress.covers(CommitSeq(1)));
    }

    /// Descriptors survive a serialization round trip.
    #[test]
    fn descriptor_round_trip() {
        let descriptor = ProjectionDescriptor::new(
            ProjectionId(9),
            "idx",
            ProjectionKind::SecondaryIndex,
            ProjectionMode::Synchronous,
            Some(4),
        );
        let bytes = bincode::serialize(&descriptor).unwrap();
        assert_eq!(
            bincode::deserialize::<ProjectionDescriptor>(&bytes).unwrap(),
            descriptor
        );
    }
}
