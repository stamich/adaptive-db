//! Native change-data capture (CDC) over the canonical log.
//!
//! The change feed is not a separate structure: it is the log itself, read through
//! [`TxAssembler`]. A consumer polls [`crate::Database::read_changes`] with a
//! [`ChangeCursor`] and receives committed transactions in commit order, each with per-row
//! `before`/`after` images. The returned `next` cursor resumes exactly after the last
//! transaction examined, so delivery is gap-free and a consumer that stores its cursor
//! together with its own side effects gets exactly-once processing. Durable named cursors are
//! available through [`crate::Database::commit_consumer_offset`].
//!
//! Only the durable prefix of the log is exposed: a consumer can never observe a commit that a
//! crash could still take back.
//!
//! Since Milestone 2.4.0 the feed also carries the catalog: a `CREATE`/`ALTER TABLE` arrives as a
//! [`CatalogChange::Schema`] with the full new schema version, a `DROP TABLE` as
//! [`CatalogChange::DropEntity`], in commit order with the row changes. Every row change names
//! the schema version its row was written under, so a consumer can interpret old and new rows
//! without asking the engine (rows are delivered as stored, never adapted). Index DDL only moves
//! the catalog epoch and is not part of the feed.

use std::{collections::BTreeSet, path::Path};

use adb_core::{CommitTs, Lsn, Row, RowId, SchemaEpoch, SchemaVersion, TxId};
use adb_schema::{CatalogEvent, EntitySchema};
use adb_tx::Mutation;
use adb_wal::{WalCursor, WalError};

use crate::{
    committed::{LoggedTx, TxAssembler},
    DbError,
};

/// Position in the change feed: a transaction boundary in the canonical log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChangeCursor(pub Lsn);

impl ChangeCursor {
    /// The start of the log.
    pub const BEGINNING: Self = ChangeCursor(Lsn(0));
}

/// What happened to a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The row did not exist (or was deleted) before.
    Insert,
    /// The row existed and was replaced.
    Update,
    /// The row existed and was removed.
    Delete,
}

/// One row-level change.
#[derive(Debug, Clone, PartialEq)]
pub struct RowChange {
    /// Storage key (`entity << 64 | primary key`).
    pub row_id: RowId,
    /// Kind of change.
    pub kind: ChangeKind,
    /// State before the transaction.
    pub before: Option<Row>,
    /// State after the transaction.
    pub after: Option<Row>,
    /// Schema version the row was written under (of `after`, or of `before` for a delete);
    /// 0 for rows written without a schema.
    pub schema_version: SchemaVersion,
}

/// A change of the catalog committed by a DDL statement.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogChange {
    /// `CREATE TABLE` (version 1) or `ALTER TABLE` (a later version) published `schema`.
    Schema {
        /// Catalog epoch the version was published at.
        epoch: SchemaEpoch,
        /// The new version, with every field ever defined.
        schema: EntitySchema,
    },
    /// `DROP TABLE` of `entity_id`.
    DropEntity {
        /// Catalog epoch of the drop.
        epoch: SchemaEpoch,
        /// Dropped entity.
        entity_id: u64,
    },
}

impl CatalogChange {
    /// Entity the change concerns.
    pub fn entity_id(&self) -> u64 {
        match self {
            CatalogChange::Schema { schema, .. } => schema.entity_id,
            CatalogChange::DropEntity { entity_id, .. } => *entity_id,
        }
    }

    /// The feed's view of one logged catalog event (`None` for an epoch-only event).
    fn from_event(epoch: SchemaEpoch, event: &CatalogEvent) -> Option<Self> {
        match event {
            CatalogEvent::Schema(schema) => Some(CatalogChange::Schema {
                epoch,
                schema: schema.clone(),
            }),
            CatalogEvent::DropEntity { entity_id } => Some(CatalogChange::DropEntity {
                epoch,
                entity_id: *entity_id,
            }),
            CatalogEvent::Epoch => None,
        }
    }
}

impl RowChange {
    /// Entity (table) of the row.
    pub fn entity_id(&self) -> u64 {
        self.row_id.entity_id()
    }

    /// SQL primary key of the row (see [`RowId::from_primary_key`]).
    pub fn primary_key(&self) -> i64 {
        self.row_id.signed_primary_key()
    }
}

/// All changes of one committed transaction that pass the filter.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeEvent {
    /// Transaction id.
    pub tx_id: TxId,
    /// Commit timestamp (the snapshot at which the changes became visible).
    pub commit_ts: CommitTs,
    /// Position of the commit record; a unique, monotonically increasing event id.
    pub commit_lsn: Lsn,
    /// Cursor that resumes right after this event (for per-event acknowledgement).
    pub cursor_after: ChangeCursor,
    /// Catalog changes of the transaction (applied before its row changes).
    pub catalog: Vec<CatalogChange>,
    /// Row changes in row order.
    pub changes: Vec<RowChange>,
}

/// A page of the change feed.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeBatch {
    /// Events in commit order.
    pub events: Vec<ChangeEvent>,
    /// Cursor to pass to the next call.
    pub next: ChangeCursor,
}

/// Which rows a consumer is interested in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeFilter {
    /// Allowed entity ids; `None` means every entity.
    entities: Option<BTreeSet<u64>>,
}

impl ChangeFilter {
    /// Every row of every entity.
    pub fn all() -> Self {
        Self::default()
    }

    /// Only rows of the given entities.
    pub fn entities(entities: impl IntoIterator<Item = u64>) -> Self {
        Self {
            entities: Some(entities.into_iter().collect()),
        }
    }

    /// Whether a change of `row_id` is delivered.
    pub fn matches(&self, row_id: RowId) -> bool {
        self.matches_entity(row_id.entity_id())
    }

    /// Whether changes of `entity_id` (rows or its schema) are delivered.
    pub fn matches_entity(&self, entity_id: u64) -> bool {
        self.entities
            .as_ref()
            .is_none_or(|entities| entities.contains(&entity_id))
    }
}

/// Reads up to `max_events` matching events from `from` up to the durable end `until`.
pub(crate) fn read_changes(
    wal_dir: &Path,
    from: ChangeCursor,
    until: Lsn,
    max_events: usize,
    filter: &ChangeFilter,
) -> Result<ChangeBatch, DbError> {
    if from.0 > until {
        return Err(DbError::InvalidArgument(format!(
            "change cursor {:?} is beyond the durable end of the log {until:?}",
            from.0
        )));
    }
    let mut cursor = WalCursor::open(wal_dir, from.0).map_err(|error| match error {
        WalError::Truncated {
            requested,
            earliest,
        } => DbError::ChangeLogTruncated {
            requested,
            earliest,
        },
        other => other.into(),
    })?;
    let mut assembler = TxAssembler::strict();
    let mut next = from;
    let mut events = Vec::new();
    let mut at_boundary = true;
    while events.len() < max_events {
        let entry = match cursor.next_before(until) {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(WalError::Corrupt(_)) if at_boundary && next == from => {
                return Err(invalid_cursor(from));
            }
            Err(error) => return Err(error.into()),
        };
        if at_boundary && !matches!(entry.record, adb_wal::WalRecord::Begin { .. }) {
            return Err(invalid_cursor(from));
        }
        at_boundary = false;
        if let Some(logged) = assembler.push(entry)? {
            next = ChangeCursor(logged.end);
            at_boundary = true;
            if let Some(event) = to_event(logged, filter) {
                events.push(event);
            }
        }
    }
    Ok(ChangeBatch { events, next })
}

/// Error returned for a cursor that does not point at a transaction boundary.
fn invalid_cursor(cursor: ChangeCursor) -> DbError {
    DbError::InvalidArgument(format!(
        "change cursor {:?} does not point at a transaction boundary",
        cursor.0
    ))
}

/// Converts a logged transaction into an event, or `None` when no change passes the filter.
fn to_event(logged: LoggedTx, filter: &ChangeFilter) -> Option<ChangeEvent> {
    let LoggedTx {
        tx,
        commit_lsn,
        end,
    } = logged;
    let before_of = |row_id: RowId| {
        tx.before_images
            .iter()
            .find(|(id, _)| *id == row_id)
            .and_then(|(_, version)| version.value.clone())
    };
    let changes: Vec<RowChange> = tx
        .mutations
        .iter()
        .filter(|(row_id, _)| filter.matches(*row_id))
        .filter_map(|(row_id, mutation)| {
            let before = before_of(*row_id);
            let after = match mutation {
                Mutation::Put(row) => Some(row.clone()),
                Mutation::Delete => None,
            };
            let kind = match (&before, &after) {
                (None, Some(_)) => ChangeKind::Insert,
                (Some(_), Some(_)) => ChangeKind::Update,
                (Some(_), None) => ChangeKind::Delete,
                (None, None) => return None, // deleting a row that did not exist
            };
            let schema_version = after
                .as_ref()
                .or(before.as_ref())
                .map_or(SchemaVersion::default(), |row| row.schema_version);
            Some(RowChange {
                row_id: *row_id,
                kind,
                before,
                after,
                schema_version,
            })
        })
        .collect();
    let catalog: Vec<CatalogChange> = tx
        .catalog
        .iter()
        .filter_map(|(epoch, event)| CatalogChange::from_event(*epoch, event))
        .filter(|change| filter.matches_entity(change.entity_id()))
        .collect();
    (!changes.is_empty() || !catalog.is_empty()).then_some(ChangeEvent {
        tx_id: tx.tx_id,
        commit_ts: tx.commit_ts,
        commit_lsn,
        cursor_after: ChangeCursor(end),
        catalog,
        changes,
    })
}
