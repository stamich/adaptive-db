//! The engine's change log: one [`ChangeEnvelope`] per commit, derived from the canonical log.
//!
//! The canonical WAL remains the source of truth. The change log is the ordered stream that
//! projections and sessions consume. It is appended on the commit path *after* the WAL append
//! and the synchronous projections, and *before* the commit is published, so every published
//! `CommitSeq` has its envelope in the file. It is fsynced lazily (checkpoint and close): after
//! a crash the WAL still holds every durable commit and the missing tail of the change log is
//! rebuilt from it at open.
use adb_change_log::{ChangeEnvelope, ChangePayload, RowChange, SchemaChangeMarker};
use adb_schema::CatalogEvent;

use crate::committed::CommittedTx;

/// Name of the change-log directory inside the database directory.
pub(crate) const CHANGE_LOG_DIR: &str = "change-log";

/// Builds the envelope of one commit.
///
/// A commit with catalog records becomes a [`ChangePayload::Schema`]; every other commit is a
/// [`ChangePayload::Data`]. (A DDL commit never carries row changes in the engine.)
pub(crate) fn envelope_of(tx: &CommittedTx) -> ChangeEnvelope {
    let payload = if tx.catalog.is_empty() {
        ChangePayload::Data {
            tx_id: tx.tx_id,
            rows: tx
                .row_images()
                .map(|(row_id, before, after)| RowChange {
                    entity_id: row_id.entity_id(),
                    row_id,
                    before_schema_version: before.as_ref().map(|row| row.schema_version),
                    after_schema_version: after.as_ref().map(|row| row.schema_version),
                    before,
                    after,
                })
                .collect(),
        }
    } else {
        ChangePayload::Schema {
            tx_id: tx.tx_id,
            changes: tx
                .catalog
                .iter()
                .map(|(_, event)| match event {
                    CatalogEvent::Schema(schema) => SchemaChangeMarker::Schema {
                        entity_id: schema.entity_id,
                        schema_version: schema.version,
                    },
                    CatalogEvent::DropEntity { entity_id } => SchemaChangeMarker::DropEntity {
                        entity_id: *entity_id,
                    },
                    CatalogEvent::Epoch => SchemaChangeMarker::Epoch,
                })
                .collect(),
        }
    };
    ChangeEnvelope {
        commit_seq: tx.commit_seq,
        commit_ts: tx.commit_ts,
        schema_epoch: tx.schema_epoch,
        payload,
    }
}
