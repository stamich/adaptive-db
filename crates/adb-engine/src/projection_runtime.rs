//! The engine's synchronous projections as seen by the projection runtime.
//!
//! The current-row store and every secondary index are updated inside the commit, so they are
//! always as far as the last commit applied. They are registered with the runtime anyway: a
//! reader asking "is this projection fresh enough for my session?" gets one answer for every
//! kind of projection, and the registry is the single place that knows what exists.
use std::{fs, path::Path};

use adb_core::{CommitSeq, IndexId, ProjectionId};
use adb_index::IndexDefinition;
use adb_projection_runtime::{
    ProjectionDescriptor, ProjectionFrontier, ProjectionKind, ProjectionMode, ProjectionRegistry,
    ProjectionState,
};

use crate::DbError;

/// Directory of the registry's checkpoint inside the database directory.
pub(crate) const PROJECTION_RUNTIME_DIR: &str = "projection-runtime";

/// Name of the current-row projection.
pub const CURRENT_ROW_PROJECTION_NAME: &str = "__current_row";

/// Id of the current-row projection.
pub const CURRENT_ROW_PROJECTION: ProjectionId = ProjectionId(1);

/// Ids of index projections start here, so they never collide with the fixed ones.
const INDEX_PROJECTION_BASE: u64 = 1_000;

/// The projection id of a secondary index.
pub fn index_projection_id(index: IndexId) -> ProjectionId {
    ProjectionId(INDEX_PROJECTION_BASE + index.0)
}

/// The projection name of a secondary index.
pub fn index_projection_name(definition: &IndexDefinition) -> String {
    format!("index:{}", definition.name)
}

/// Descriptor of the current-row projection.
fn current_row_descriptor() -> ProjectionDescriptor {
    ProjectionDescriptor::new(
        CURRENT_ROW_PROJECTION,
        CURRENT_ROW_PROJECTION_NAME,
        ProjectionKind::CurrentRow,
        ProjectionMode::Synchronous,
        None,
    )
}

/// Descriptor of the projection of `definition`.
fn index_descriptor(definition: &IndexDefinition) -> ProjectionDescriptor {
    ProjectionDescriptor::new(
        index_projection_id(definition.id),
        index_projection_name(definition),
        ProjectionKind::SecondaryIndex,
        ProjectionMode::Synchronous,
        Some(definition.entity_id),
    )
}

/// Opens the registry persisted in the database directory.
pub(crate) fn open_registry(dir: &Path) -> Result<ProjectionRegistry, DbError> {
    let path = dir.join(PROJECTION_RUNTIME_DIR);
    fs::create_dir_all(&path)?;
    Ok(ProjectionRegistry::open(path)?)
}

/// Makes the registry describe exactly the synchronous projections that exist: the current-row
/// store and one projection per index of the catalog. Stale entries (an index dropped since the
/// last checkpoint) are removed; every synchronous projection is then brought to `head`, which
/// is where recovery left the stores. Asynchronous projections are left as they were saved.
pub(crate) fn reconcile(
    registry: &ProjectionRegistry,
    indexes: &[IndexDefinition],
    head: CommitSeq,
) -> Result<(), DbError> {
    let ready = ProjectionFrontier::at(head);
    registry.register(current_row_descriptor(), ProjectionState::Ready, ready)?;
    let wanted: Vec<_> = indexes.iter().map(|d| index_projection_id(d.id)).collect();
    for progress in registry.list() {
        if progress.descriptor.kind == ProjectionKind::SecondaryIndex
            && !wanted.contains(&progress.descriptor.id)
        {
            registry.remove(progress.descriptor.id);
        }
    }
    for definition in indexes {
        register_index(registry, definition, head)?;
    }
    registry.set_head(head);
    registry.advance_synchronous(head);
    Ok(())
}

/// Registers the projection of a new (or restored) index, ready at `head`.
pub(crate) fn register_index(
    registry: &ProjectionRegistry,
    definition: &IndexDefinition,
    head: CommitSeq,
) -> Result<(), DbError> {
    registry.register(
        index_descriptor(definition),
        ProjectionState::Ready,
        ProjectionFrontier::at(head),
    )?;
    Ok(())
}
