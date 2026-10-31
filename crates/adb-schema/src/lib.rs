//! Schema evolution of Adaptive DB (Milestone 2.4.0): versioned entity schemas, the changes
//! that produce new versions, their validation, and the adapter that reads a row written under
//! one version as another.
//!
//! The engine owns every schema (the JVM catalog is a cache of it) and keeps their history in
//! the canonical log; this crate holds the pure, I/O-free part of that design:
//!
//! * [`model`] — [`EntitySchema`] (one immutable version of an entity) and [`FieldDef`];
//! * [`change`] — [`EntitySpec`] (`CREATE TABLE`) and [`SchemaChange`] (`ALTER TABLE`), applied
//!   with [`create`] and [`apply`];
//! * [`validate`] — the rules of one version and of a chain of versions;
//! * [`limits`] — size bounds;
//! * [`error`] — [`SchemaError`].
//!
//! Invariants: a published version is never changed; versions of one entity form a chain
//! `1, 2, 3, ...`; a [`adb_core::FieldId`] keeps its identity across renames and is never handed
//! out again after its field is dropped; the primary key can be renamed but never dropped.
pub mod change;
pub mod error;
pub mod limits;
pub mod model;
pub mod validate;

pub use change::{apply, create, EntitySpec, NewField, SchemaChange};
pub use error::SchemaError;
pub use model::{EntitySchema, FieldDef, ForeignKey, SYSTEM_ENTITY_FIELD};
