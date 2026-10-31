//! Reading a row written under one schema version as another.
//!
//! A [`RowAdapter`] is compiled once per `(entity, from, to)` and applied to every row stamped
//! `from`. It is a short list of operations:
//!
//! * **default**: a field of the target version that the row *predates* (the row was written
//!   before the field was introduced) and that has a default reads that default: a virtual
//!   default, nothing is rewritten;
//! * **hide**: a field that is not active in the target version (dropped, or added after it)
//!   is removed.
//!
//! A rename needs no operation: rows hold field ids, not names.
//!
//! The default rule compares versions, not presence: a field the row was written *with* (at or
//! after its introduction) that is absent reads NULL, and an explicit [`Value::Null`] stays NULL.
//! A sparse row may omit its NULLs, so absence alone could not tell "predates the field" from
//! "is NULL".
//!
//! Adaptation works in both directions. Reading forward (an old row through the latest schema)
//! is the normal case; reading backward happens when a running query pinned an older version and
//! a concurrent `ALTER TABLE` lets newer rows appear: fields added later are hidden, and a field
//! the row was written without because it was already dropped reads NULL.
use adb_core::{FieldId, Row, SchemaVersion, Value};

use crate::{model::SYSTEM_ENTITY_FIELD, EntitySchema};

/// The compiled adaptation of rows stamped `from` to version `to` of one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct RowAdapter {
    /// Effective version of the rows it adapts (unversioned rows count as version 1).
    from: SchemaVersion,
    /// Target version.
    to: SchemaVersion,
    /// Fields active in `to`, in id order (every other field is hidden).
    keep: Vec<FieldId>,
    /// Defaults of `to`'s fields that rows stamped `from` predate.
    defaults: Vec<(FieldId, Value)>,
}

impl RowAdapter {
    /// Compiles the adaptation from `from` to `to` using `history`, any version of the entity at
    /// or after `to` (every version holds every field ever added, with its introducing and
    /// dropping versions).
    ///
    /// # Panics
    ///
    /// If `to` is newer than `history`: a caller adapts only to versions it knows.
    pub fn compile(history: &EntitySchema, from: SchemaVersion, to: SchemaVersion) -> Self {
        assert!(
            to <= history.version && !to.is_unversioned(),
            "adapting to version {} with history up to {}",
            to.0,
            history.version.0
        );
        let from = from.effective();
        let mut keep = Vec::new();
        let mut defaults = Vec::new();
        for field in history.fields.iter().filter(|field| field.is_active_in(to)) {
            keep.push(field.id);
            if from < field.introduced_in {
                if let Some(default) = &field.default {
                    defaults.push((field.id, default.clone()));
                }
            }
        }
        Self {
            from,
            to,
            keep,
            defaults,
        }
    }

    /// Whether the adapter changes nothing but the stamp (the row is already at the target
    /// version, the common case).
    pub fn is_identity(&self) -> bool {
        self.from == self.to
    }

    /// The target version.
    pub fn target(&self) -> SchemaVersion {
        self.to
    }

    /// `row` (stamped `from`) as a row of version `to`. Rows already at `to` are returned as
    /// they are, apart from the stamp: they were validated against it when written.
    pub fn adapt(&self, mut row: Row) -> Row {
        if !self.is_identity() {
            row.fields.retain(|field, _| {
                *field == SYSTEM_ENTITY_FIELD || self.keep.binary_search(field).is_ok()
            });
            for (field, value) in &self.defaults {
                row.fields.entry(*field).or_insert_with(|| value.clone());
            }
        }
        if !row.schema_version.is_unversioned() || !self.is_identity() {
            row.schema_version = self.to;
        }
        row
    }
}
