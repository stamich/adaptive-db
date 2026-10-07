//! Opening a database: finish an interrupted checkpoint, then replay the log tail.
//!
//! 1. A committed checkpoint journal left by a crash is re-applied ([`Journal::recover`]), so
//!    the projections and the checkpoint record are exactly the last checkpointed state.
//! 2. The log is read from the checkpoint's `replay_from` position and every committed
//!    transaction is applied to the projections (the buffer pool is no-steal, so nothing after
//!    the checkpoint ever reached the projection files).
//! 3. A fresh checkpoint is written if anything was replayed.
//!
//! Recovery cost is therefore proportional to the log written since the last checkpoint.

use std::path::Path;

use adb_core::{CommitTs, Lsn};
use adb_journal::Journal;
use adb_storage::{Checkpoint, CheckpointStore, Checkpointable};
use adb_wal::{earliest_lsn, WalCursor, WalRecord};

use crate::{committed::TxAssembler, projections::Projections, DatabaseOptions, DbError};

/// State reconstructed by [`recover`].
pub(crate) struct Recovered {
    pub projections: Projections,
    pub last_commit_ts: CommitTs,
    pub last_tx_id: u64,
    pub vacuumed_through: CommitTs,
}

/// Publishes the projections' dirty pages and `checkpoint` atomically.
pub(crate) fn write_checkpoint(
    journal: &Journal,
    store: &CheckpointStore,
    projections: &Projections,
    checkpoint: &Checkpoint,
) -> Result<(), DbError> {
    let mut writes = projections.journal_writes()?;
    writes.push(store.journal_write(checkpoint)?);
    journal.commit(&writes)?;
    projections.mark_clean();
    Ok(())
}

/// Brings the projections of the database in `dir` up to the end of its log.
pub(crate) fn recover(
    dir: &Path,
    wal_dir: &Path,
    options: &DatabaseOptions,
    journal: &Journal,
    store: &CheckpointStore,
) -> Result<Recovered, DbError> {
    journal.recover()?;
    let checkpoint = store.load()?;
    let projections = Projections::open(dir, options.buffer_pages)?;

    // Databases migrated from format 2 re-read the whole retained log and skip what is applied.
    let start = match checkpoint.applied_through {
        Some(_) => earliest_lsn(wal_dir)?.unwrap_or(Lsn(0)),
        None => checkpoint.replay_from,
    };
    let mut cursor = WalCursor::open(wal_dir, start)?;
    let mut assembler = TxAssembler::tolerating_orphans_through(checkpoint.applied_through);
    let mut state = Recovered {
        projections,
        last_commit_ts: checkpoint.last_commit_ts,
        last_tx_id: checkpoint.last_tx_id,
        vacuumed_through: checkpoint.vacuumed_through,
    };
    let mut replayed = checkpoint.applied_through.is_some();
    let mut end = start;

    while let Some(entry) = cursor.next_before(Lsn(u64::MAX))? {
        end = entry.next;
        state.last_tx_id = state.last_tx_id.max(tx_id_of(&entry.record));
        let Some(logged) = assembler.push(entry)? else {
            continue;
        };
        if checkpoint
            .applied_through
            .is_some_and(|applied| logged.commit_lsn <= applied)
        {
            continue;
        }
        state.projections.apply(&logged.tx, logged.commit_lsn)?;
        state.last_commit_ts = state.last_commit_ts.max(logged.tx.commit_ts);
        replayed = true;

        if state.projections.dirty_pages() > options.checkpoint_dirty_pages
            && !assembler.is_mid_transaction()
        {
            write_checkpoint(
                journal,
                store,
                &state.projections,
                &state.checkpoint(logged.end),
            )?;
        }
    }

    if replayed {
        write_checkpoint(journal, store, &state.projections, &state.checkpoint(end))?;
    }
    Ok(state)
}

impl Recovered {
    fn checkpoint(&self, replay_from: Lsn) -> Checkpoint {
        Checkpoint {
            replay_from,
            last_commit_ts: self.last_commit_ts,
            last_tx_id: self.last_tx_id,
            vacuumed_through: self.vacuumed_through,
            ..Checkpoint::default()
        }
    }
}

fn tx_id_of(record: &WalRecord) -> u64 {
    match record {
        WalRecord::Begin { tx_id, .. }
        | WalRecord::Version { tx_id, .. }
        | WalRecord::Put { tx_id, .. }
        | WalRecord::Delete { tx_id, .. }
        | WalRecord::Commit { tx_id, .. }
        | WalRecord::Abort { tx_id } => tx_id.0,
    }
}
