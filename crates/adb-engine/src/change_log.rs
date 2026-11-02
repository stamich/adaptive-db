//! The engine's change log: one [`ChangeEnvelope`] per commit, derived from the canonical log.
//!
//! The canonical WAL remains the source of truth. The change log is the ordered stream that
//! projections and sessions consume. It is appended on the commit path *after* the WAL append
//! and the synchronous projections, and *before* the commit is published, so every published
//! `CommitSeq` has its envelope in the file. It is fsynced lazily (checkpoint and close): after
//! a crash the WAL still holds every durable commit and the missing tail of the change log is
//! rebuilt from it at open.
use std::{fs, path::Path};

use adb_change_log::{
    ChangeEnvelope, ChangeLog, ChangeLogError, ChangePayload, RowChange, SchemaChangeMarker,
    DEFAULT_SEGMENT_BYTES,
};
use adb_core::{CommitSeq, Lsn};
use adb_schema::CatalogEvent;
use adb_storage::Checkpoint;
use adb_wal::{earliest_lsn, WalCursor, WalError};

use crate::{committed::CommittedTx, committed::TxAssembler, DbError};

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

/// Opens the change log in `dir`; a log that fails validation (damage that is not a torn tail)
/// is discarded and starts empty, because [`repair`] rebuilds it from the canonical log.
pub(crate) fn open_or_reset(dir: &Path) -> Result<ChangeLog, DbError> {
    match ChangeLog::open(dir, DEFAULT_SEGMENT_BYTES) {
        Ok(log) => Ok(log),
        Err(ChangeLogError::Corrupt { .. }) => {
            fs::remove_dir_all(dir)?;
            Ok(ChangeLog::open(dir, DEFAULT_SEGMENT_BYTES)?)
        }
        Err(other) => Err(other.into()),
    }
}

/// Brings the change log in line with the canonical log after recovery.
///
/// * Envelopes above `wal_head` exist only if a crash took the WAL tail but not the
///   change-log tail; they describe commits that never became durable and are cut off.
/// * Commits the change log lacks (its lazily fsynced tail was lost, the directory is new
///   because the database was written by an older engine, or the log was discarded as damaged)
///   are rebuilt from the WAL, data and catalog commits in their one commit order.
///
/// `before` is the checkpoint as it was before recovery: when the change log already covers
/// everything up to it, only the log from `before.replay_from` is read; otherwise the whole
/// retained log is.
pub(crate) fn repair(
    wal_dir: &Path,
    log: &ChangeLog,
    before: &Checkpoint,
    wal_head: CommitSeq,
) -> Result<(), DbError> {
    log.truncate_after(wal_head)?;
    let head = log.head().unwrap_or(CommitSeq::ZERO);
    if head >= wal_head {
        return Ok(());
    }
    let covered = before.applied_through.is_none() && head >= before.last_commit_seq;
    let start = if covered {
        before.replay_from
    } else {
        earliest_lsn(wal_dir)?.unwrap_or(Lsn(0))
    };
    let mut cursor = WalCursor::open(wal_dir, start)?;
    let mut assembler = TxAssembler::tolerating_orphans_through(before.applied_through);
    while let Some(entry) = cursor.next_before(Lsn(u64::MAX))? {
        if let Some(logged) = assembler.push(entry)? {
            if logged.tx.commit_seq > head {
                log.append(&envelope_of(&logged.tx))?;
            }
        }
    }
    log.sync()?;
    if log.head().unwrap_or(CommitSeq::ZERO) != wal_head {
        return Err(WalError::Corrupt(format!(
            "the change log could not be rebuilt to commit {} (reached {})",
            wal_head.0,
            log.head().map_or(0, |seq| seq.0)
        ))
        .into());
    }
    Ok(())
}
