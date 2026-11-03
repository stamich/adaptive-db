//! The projection registry: who exists, how far each has got, and what it costs to wait.
//!
//! The registry is a cheap, cloneable handle around one mutex-protected table. Frontiers move
//! forward only. Changes to *what exists or in which state* bump the registry's metadata epoch
//! (a session can tell that its picture of the projections is stale); moving a frontier does not.
//! [`ProjectionRegistry::checkpoint`] persists the table; the lag figures and the rate are
//! runtime observations and are not persisted.
use std::{path::Path, sync::Arc};

use adb_core::{CommitSeq, ProjectionEpoch, ProjectionId};
use parking_lot::{Condvar, Mutex};

use crate::{
    checkpoint::{CheckpointEntry, ProjectionCheckpoint, ProjectionCheckpointStore},
    error::ProjectionError,
    model::{ProjectionDescriptor, ProjectionFrontier, ProjectionProgress, ProjectionState},
};

/// Weight of a new observation in the smoothed consumption rate.
const RATE_SMOOTHING: f64 = 0.3;

/// One registered projection and its runtime observations.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// Definition.
    pub descriptor: ProjectionDescriptor,
    /// Lifecycle state.
    pub state: ProjectionState,
    /// Consumed position.
    pub frontier: ProjectionFrontier,
    /// Changes between the frontier and the head, as last reported.
    pub lag_changes: u64,
    /// Encoded bytes between the frontier and the head, as last reported.
    pub lag_bytes: u64,
    /// Smoothed consumption speed in commits per second.
    pub rate: f64,
}

/// The table behind the handle.
#[derive(Debug)]
pub(crate) struct Table {
    /// Metadata epoch.
    pub epoch: ProjectionEpoch,
    /// Head of the commit order, for lag figures.
    pub head: CommitSeq,
    /// Registered projections in registration order.
    pub entries: Vec<Entry>,
}

/// State shared by all clones of the registry.
pub(crate) struct Shared {
    /// The table.
    pub table: Mutex<Table>,
    /// Signalled whenever a frontier, the head or a state changes.
    pub changed: Condvar,
    /// Where the table is persisted, if anywhere.
    pub store: Option<ProjectionCheckpointStore>,
}

/// Handle to the registry of one database.
#[derive(Clone)]
pub struct ProjectionRegistry {
    /// Shared state.
    pub(crate) shared: Arc<Shared>,
}

impl std::fmt::Debug for ProjectionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let table = self.shared.table.lock();
        f.debug_struct("ProjectionRegistry")
            .field("epoch", &table.epoch)
            .field("projections", &table.entries.len())
            .finish()
    }
}

impl ProjectionRegistry {
    /// A registry that is never persisted.
    pub fn in_memory() -> Self {
        Self::from_parts(ProjectionCheckpoint::default(), None)
    }

    /// Opens the registry persisted in `dir`, restoring the projections, their states and
    /// frontiers from the last checkpoint.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, ProjectionError> {
        let store = ProjectionCheckpointStore::new(dir);
        let checkpoint = store.load()?;
        Ok(Self::from_parts(checkpoint, Some(store)))
    }

    /// Builds a registry from a loaded checkpoint.
    fn from_parts(
        checkpoint: ProjectionCheckpoint,
        store: Option<ProjectionCheckpointStore>,
    ) -> Self {
        let entries = checkpoint
            .entries
            .into_iter()
            .map(|entry| Entry {
                descriptor: entry.descriptor,
                state: entry.state,
                frontier: entry.frontier,
                lag_changes: 0,
                lag_bytes: 0,
                rate: 0.0,
            })
            .collect();
        Self {
            shared: Arc::new(Shared {
                table: Mutex::new(Table {
                    epoch: checkpoint.epoch,
                    head: CommitSeq::ZERO,
                    entries,
                }),
                changed: Condvar::new(),
                store,
            }),
        }
    }

    /// Metadata epoch: moves when a projection is registered, replaced, removed or changes state.
    pub fn epoch(&self) -> ProjectionEpoch {
        self.shared.table.lock().epoch
    }

    /// Registers a projection with an initial state and frontier.
    ///
    /// Registering the identical definition again is a no-op that leaves state and frontier
    /// alone (so a restart can re-register everything it knows). A different definition under an
    /// existing id or name is an error; use [`ProjectionRegistry::redefine`] to replace one.
    pub fn register(
        &self,
        descriptor: ProjectionDescriptor,
        state: ProjectionState,
        frontier: ProjectionFrontier,
    ) -> Result<(), ProjectionError> {
        descriptor.validate()?;
        if !frontier.is_valid() {
            return Err(ProjectionError::Invalid(
                "the delta frontier must not trail the base frontier".into(),
            ));
        }
        let mut table = self.shared.table.lock();
        if let Some(existing) = table
            .entries
            .iter()
            .find(|e| e.descriptor.id == descriptor.id || e.descriptor.name == descriptor.name)
        {
            return if same_definition(&existing.descriptor, &descriptor) {
                Ok(())
            } else {
                Err(ProjectionError::Invalid(format!(
                    "projection {} conflicts with the registered {}",
                    descriptor.name, existing.descriptor.name
                )))
            };
        }
        table.entries.push(Entry {
            descriptor,
            state,
            frontier,
            lag_changes: 0,
            lag_bytes: 0,
            rate: 0.0,
        });
        bump(&mut table);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// Replaces the definition of an existing projection (same id). The projection's
    /// definition epoch moves to one above the old one, its state becomes
    /// [`ProjectionState::Building`] and its frontier restarts at zero: whatever it held was
    /// built for the old definition.
    pub fn redefine(&self, mut descriptor: ProjectionDescriptor) -> Result<(), ProjectionError> {
        descriptor.validate()?;
        let mut table = self.shared.table.lock();
        let entry = table
            .entries
            .iter_mut()
            .find(|e| e.descriptor.id == descriptor.id)
            .ok_or_else(|| unknown(descriptor.id))?;
        descriptor.epoch = ProjectionEpoch(entry.descriptor.epoch.0 + 1);
        entry.descriptor = descriptor;
        entry.state = ProjectionState::Building;
        entry.frontier = ProjectionFrontier::at(CommitSeq::ZERO);
        bump(&mut table);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// Removes a projection; returns whether it existed.
    pub fn remove(&self, id: ProjectionId) -> bool {
        let mut table = self.shared.table.lock();
        let before = table.entries.len();
        table.entries.retain(|e| e.descriptor.id != id);
        let removed = table.entries.len() != before;
        if removed {
            bump(&mut table);
            self.shared.changed.notify_all();
        }
        removed
    }

    /// Progress of one projection.
    pub fn get(&self, id: ProjectionId) -> Option<ProjectionProgress> {
        let table = self.shared.table.lock();
        table
            .entries
            .iter()
            .find(|e| e.descriptor.id == id)
            .map(|entry| progress_of(entry, table.head))
    }

    /// Progress of the projection called `name`.
    pub fn find(&self, name: &str) -> Option<ProjectionProgress> {
        let table = self.shared.table.lock();
        table
            .entries
            .iter()
            .find(|e| e.descriptor.name == name)
            .map(|entry| progress_of(entry, table.head))
    }

    /// Progress of every projection, ordered by id.
    pub fn list(&self) -> Vec<ProjectionProgress> {
        let table = self.shared.table.lock();
        let mut all: Vec<_> = table
            .entries
            .iter()
            .map(|entry| progress_of(entry, table.head))
            .collect();
        all.sort_by_key(|p| p.descriptor.id);
        all
    }

    /// Records the head of the commit order (the newest published commit). Never moves back.
    pub fn set_head(&self, head: CommitSeq) {
        let mut table = self.shared.table.lock();
        if head > table.head {
            table.head = head;
            self.shared.changed.notify_all();
        }
    }

    /// Moves the base frontier of `id` to `to` (the delta frontier follows when it would trail).
    ///
    /// Frontiers only move forward: moving back is an error, repeating the current position is
    /// accepted. Only a [`ProjectionState::Building`] or [`ProjectionState::Ready`] projection
    /// can advance.
    pub fn advance(&self, id: ProjectionId, to: CommitSeq) -> Result<(), ProjectionError> {
        let mut table = self.shared.table.lock();
        let entry = table
            .entries
            .iter_mut()
            .find(|e| e.descriptor.id == id)
            .ok_or_else(|| unknown(id))?;
        if !matches!(
            entry.state,
            ProjectionState::Building | ProjectionState::Ready
        ) {
            return Err(ProjectionError::Invalid(format!(
                "projection {} is {:?} and cannot advance",
                entry.descriptor.name, entry.state
            )));
        }
        if to < entry.frontier.base_commit_seq {
            return Err(ProjectionError::Invalid(format!(
                "projection {} cannot move back from {} to {}",
                entry.descriptor.name, entry.frontier.base_commit_seq.0, to.0
            )));
        }
        entry.frontier.base_commit_seq = to;
        entry.frontier.delta_commit_seq = entry.frontier.delta_commit_seq.max(to);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// Records how many changes and encoded bytes `id` still has to consume.
    pub fn report_lag(
        &self,
        id: ProjectionId,
        changes: u64,
        bytes: u64,
    ) -> Result<(), ProjectionError> {
        let mut table = self.shared.table.lock();
        let entry = table
            .entries
            .iter_mut()
            .find(|e| e.descriptor.id == id)
            .ok_or_else(|| unknown(id))?;
        entry.lag_changes = changes;
        entry.lag_bytes = bytes;
        Ok(())
    }

    /// Feeds the smoothed consumption speed of `id` with a new observation (commits/second).
    /// NaN, infinite and negative values are rejected so one bad measurement cannot poison the
    /// estimate that wait-or-fallback decisions are based on.
    pub fn observe_rate(
        &self,
        id: ProjectionId,
        commits_per_second: f64,
    ) -> Result<(), ProjectionError> {
        if !commits_per_second.is_finite() || commits_per_second < 0.0 {
            return Err(ProjectionError::Invalid(format!(
                "rate {commits_per_second} is not a finite, non-negative number"
            )));
        }
        let mut table = self.shared.table.lock();
        let entry = table
            .entries
            .iter_mut()
            .find(|e| e.descriptor.id == id)
            .ok_or_else(|| unknown(id))?;
        entry.rate = if entry.rate == 0.0 {
            commits_per_second
        } else {
            RATE_SMOOTHING * commits_per_second + (1.0 - RATE_SMOOTHING) * entry.rate
        };
        Ok(())
    }

    /// Persists descriptors, states, frontiers and the metadata epoch. A no-op for an
    /// in-memory registry.
    pub fn checkpoint(&self) -> Result<(), ProjectionError> {
        let Some(store) = &self.shared.store else {
            return Ok(());
        };
        let snapshot = {
            let table = self.shared.table.lock();
            ProjectionCheckpoint {
                epoch: table.epoch,
                entries: table
                    .entries
                    .iter()
                    .map(|e| CheckpointEntry {
                        descriptor: e.descriptor.clone(),
                        state: e.state,
                        frontier: e.frontier,
                    })
                    .collect(),
            }
        };
        store.save(&snapshot)
    }
}

/// Whether two descriptors define the same projection (the definition epoch is bookkeeping).
fn same_definition(a: &ProjectionDescriptor, b: &ProjectionDescriptor) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.kind == b.kind
        && a.mode == b.mode
        && a.entity_id == b.entity_id
}

/// Moves the metadata epoch forward.
pub(crate) fn bump(table: &mut Table) {
    table.epoch = ProjectionEpoch(table.epoch.0 + 1);
}

/// The error for an id that is not registered.
pub(crate) fn unknown(id: ProjectionId) -> ProjectionError {
    ProjectionError::Invalid(format!("projection {} is not registered", id.0))
}

/// Builds the reader's view of one entry.
pub(crate) fn progress_of(entry: &Entry, head: CommitSeq) -> ProjectionProgress {
    ProjectionProgress {
        descriptor: entry.descriptor.clone(),
        state: entry.state,
        frontier: entry.frontier,
        head_commit_seq: head,
        lag_commits: head.0.saturating_sub(entry.frontier.readable_through().0),
        lag_changes: entry.lag_changes,
        lag_bytes: entry.lag_bytes,
        commits_per_second: entry.rate,
    }
}
