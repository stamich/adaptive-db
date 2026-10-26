//! Index definitions: what an index covers, its lifecycle state, validation and limits.
use std::collections::HashSet;

use adb_core::{CommitTs, Direction, FieldId, IndexId, Row, Value};
use serde::{Deserialize, Serialize};

use crate::IndexError;

/// Longest index name, in bytes.
pub const MAX_INDEX_NAME_BYTES: usize = 128;
/// Most key columns of one index.
pub const MAX_KEY_COLUMNS: usize = 16;
/// Most covered (`INCLUDE`) columns of one index.
pub const MAX_INCLUDE_COLUMNS: usize = 32;
/// Longest encoded key of one row (the key columns, without row id and timestamp), in bytes.
/// A commit whose row would exceed it is rejected before it reaches the log.
pub const MAX_INDEX_KEY_BYTES: usize = 512;
/// Longest encoded covered payload of one row, in bytes (same rule as the key).
pub const MAX_INCLUDE_BYTES: usize = 1024;

/// One key column: a stored field and its sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexColumn {
    /// Field of the entity.
    pub field_id: FieldId,
    /// Sort direction of the column in the index.
    #[serde(default)]
    pub direction: Direction,
}

/// What a client asks for with `CREATE INDEX` (before the engine assigns an id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexSpec {
    /// Name, unique in the database (`[A-Za-z_][A-Za-z0-9_]*`, compared case-insensitively).
    pub name: String,
    /// Entity (table) indexed.
    pub entity_id: u64,
    /// Key columns, most significant first.
    pub columns: Vec<IndexColumn>,
    /// Covered columns stored in the index so that queries reading only key and covered
    /// columns need no row fetch.
    #[serde(default)]
    pub include: Vec<FieldId>,
}

/// Lifecycle of an index.
///
/// `Building` exists only while `CREATE INDEX` holds the commit lock: the definition and the
/// built index are published together by one checkpoint, which marks the index `Active`, so a
/// crash during a build leaves no trace. A drop removes the definition by a checkpoint and then
/// the files, so no persistent `Dropping` state is needed either (leftover files of a crash in
/// between are removed when the database is opened).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    /// Being built; not yet visible to queries.
    Building,
    /// Maintained by every commit and usable by queries.
    Active,
}

/// A validated index definition as stored in the engine catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexDefinition {
    /// Engine-assigned id.
    pub id: IndexId,
    /// Name as given (unique, case-insensitive).
    pub name: String,
    /// Entity indexed.
    pub entity_id: u64,
    /// Key columns.
    pub columns: Vec<IndexColumn>,
    /// Covered columns.
    pub include: Vec<FieldId>,
    /// Lifecycle state.
    pub state: IndexState,
    /// Latest commit included when the index was built: it holds entries for snapshots from
    /// this timestamp on (older snapshots must scan the table).
    pub created_at_ts: CommitTs,
}

impl IndexSpec {
    /// Checks the name and the column lists against the limits.
    pub fn validate(&self) -> Result<(), IndexError> {
        let fail = |message: String| Err(IndexError::InvalidDefinition(message));
        if self.name.is_empty() || self.name.len() > MAX_INDEX_NAME_BYTES {
            return fail(format!(
                "index name must have 1..={MAX_INDEX_NAME_BYTES} bytes"
            ));
        }
        let mut chars = self.name.chars();
        let valid_start = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !valid_start || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return fail(format!(
                "index name {:?} must match [A-Za-z_][A-Za-z0-9_]*",
                self.name
            ));
        }
        if self.columns.is_empty() || self.columns.len() > MAX_KEY_COLUMNS {
            return fail(format!(
                "an index needs 1..={MAX_KEY_COLUMNS} key columns, got {}",
                self.columns.len()
            ));
        }
        if self.include.len() > MAX_INCLUDE_COLUMNS {
            return fail(format!(
                "at most {MAX_INCLUDE_COLUMNS} INCLUDE columns, got {}",
                self.include.len()
            ));
        }
        let mut seen = HashSet::new();
        for field in self
            .columns
            .iter()
            .map(|column| column.field_id)
            .chain(self.include.iter().copied())
        {
            if !seen.insert(field) {
                return fail(format!("field {field} appears twice in the index"));
            }
        }
        Ok(())
    }
}

impl IndexDefinition {
    /// The definition of `spec` with id `id`, in state `state`, built at `created_at_ts`.
    pub fn new(id: IndexId, spec: IndexSpec, state: IndexState, created_at_ts: CommitTs) -> Self {
        Self {
            id,
            name: spec.name,
            entity_id: spec.entity_id,
            columns: spec.columns,
            include: spec.include,
            state,
            created_at_ts,
        }
    }

    /// Directions of the key columns, in order.
    pub fn directions(&self) -> Vec<Direction> {
        self.columns.iter().map(|column| column.direction).collect()
    }

    /// Values of the key columns of `row` (absent fields are NULL).
    pub fn key_values(&self, row: &Row) -> Vec<Value> {
        self.columns
            .iter()
            .map(|column| row.get(column.field_id).cloned().unwrap_or(Value::Null))
            .collect()
    }

    /// Values of the covered columns of `row` (absent fields are NULL).
    pub fn included_values(&self, row: &Row) -> Vec<Value> {
        self.include
            .iter()
            .map(|field| row.get(*field).cloned().unwrap_or(Value::Null))
            .collect()
    }

    /// Whether `field` is a key or covered column.
    pub fn covers(&self, field: FieldId) -> bool {
        self.columns.iter().any(|column| column.field_id == field) || self.include.contains(&field)
    }
}
