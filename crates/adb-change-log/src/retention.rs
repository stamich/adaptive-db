//! Retention: which part of the change log may be deleted.
//!
//! Deletion works on whole segments, oldest first, and never touches the active (newest)
//! segment. Two forces decide:
//!
//! * the [`RetentionFrontier`] — the lowest commit any consumer still needs. Segments entirely
//!   below it are *consumed* and are always removed;
//! * the [`ChangeRetentionPolicy`] — hard limits (size, age, lag) that bound the log even when a
//!   slow or dead consumer holds the frontier back. Segments removed because of a limit can
//!   still be needed by a consumer; it then finds its cursor [`Truncated`](crate::ChangeLogError::Truncated)
//!   and must rebuild, which is reported through [`PruneReport::overtook_frontier`].
use std::{fs, time::SystemTime};

use adb_core::CommitSeq;

use crate::{
    error::ChangeLogError,
    log::{sync_dir, ChangeLog},
};

/// The lowest commit sequence number some consumer still has to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionFrontier {
    /// Commits below this number are not needed by anyone.
    pub minimum_required_commit_seq: CommitSeq,
}

/// Hard limits on the log; `None` leaves a dimension unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangeRetentionPolicy {
    /// Maximum total size of all segments in bytes.
    pub max_bytes: Option<u64>,
    /// Maximum age of a segment in seconds, measured from its last modification.
    pub max_age_seconds: Option<u64>,
    /// Maximum distance in commits between the head and the first commit of a kept segment.
    pub max_lag_commits: Option<u64>,
}

/// What a [`ChangeLog::prune`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    /// Segment files deleted.
    pub segments_removed: usize,
    /// Bytes those files held.
    pub bytes_freed: u64,
    /// Oldest commit stored afterwards, `None` for an empty log.
    pub earliest_after: Option<CommitSeq>,
    /// Whether a limit removed commits that the frontier still required.
    pub overtook_frontier: bool,
}

impl ChangeLog {
    /// Total size of all segments in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.state.lock().segments.iter().map(|s| s.len).sum()
    }

    /// Deletes the oldest segments that are consumed or that exceed a limit of `policy`.
    pub fn prune(
        &self,
        frontier: RetentionFrontier,
        policy: &ChangeRetentionPolicy,
    ) -> Result<PruneReport, ChangeLogError> {
        let mut state = self.state.lock();
        let head = state.segments.last().map(|s| s.last_seq);
        let now = SystemTime::now();
        let mut total: u64 = state.segments.iter().map(|s| s.len).sum();
        let mut remove = 0usize;
        let mut overtook = false;
        // Never the last segment: it is the append target and the head's only record.
        for segment in state
            .segments
            .iter()
            .take(state.segments.len().saturating_sub(1))
        {
            let consumed = segment.last_seq < frontier.minimum_required_commit_seq;
            let too_big = policy.max_bytes.is_some_and(|max| total > max);
            let too_old = policy.max_age_seconds.is_some_and(|max| {
                fs::metadata(&segment.path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age.as_secs() > max)
            });
            let too_far = policy.max_lag_commits.is_some_and(|max| {
                head.is_some_and(|head| head.0.saturating_sub(segment.first_seq.0) > max)
            });
            if !(consumed || too_big || too_old || too_far) {
                break;
            }
            overtook |= !consumed;
            total -= segment.len;
            remove += 1;
        }
        let removed: Vec<_> = state.segments.drain(..remove).collect();
        let mut bytes_freed = 0;
        for segment in &removed {
            fs::remove_file(&segment.path)?;
            bytes_freed += segment.len;
        }
        if !removed.is_empty() {
            sync_dir(&state.dir)?;
        }
        Ok(PruneReport {
            segments_removed: removed.len(),
            bytes_freed,
            earliest_after: state.segments.first().map(|s| s.first_seq),
            overtook_frontier: overtook,
        })
    }
}
