//! Projection runtime: what is derived from the change log, and how far each derivation got.
//!
//! A *projection* is a structure maintained from the committed changes: the current-row store
//! and the secondary indexes (synchronous, updated inside the commit), or any consumer that reads
//! the change log at its own pace (asynchronous). The runtime keeps, for every projection, a
//! [`ProjectionFrontier`] — the commit sequence number it has consumed — together with its state
//! and lag. Readers use the frontiers to decide whether a projection is fresh enough for a
//! session; the change log's retention uses them to know what history must be kept.
//!
//! Frontiers are durable (see [`checkpoint`]) so a restart does not forget how far an
//! asynchronous projection got.
pub mod error;
pub mod model;

pub use error::ProjectionError;
pub use model::{
    ProjectionDescriptor, ProjectionFrontier, ProjectionKind, ProjectionMode, ProjectionProgress,
    ProjectionState, MAX_PROJECTION_NAME_BYTES,
};
