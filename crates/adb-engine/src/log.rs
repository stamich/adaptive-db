//! Appender of the canonical log with group commit.
//!
//! Appending and syncing are separate steps. Committers append under the engine's commit lock,
//! release it, and then call [`LogWriter::sync_through`]. One committer at a time performs an
//! `fsync` covering every record appended so far; others keep appending in the meantime and
//! then either find their records already durable or share the next `fsync`. Under
//! concurrency many commits therefore share one disk flush.

use std::path::Path;

use adb_core::Lsn;
use adb_wal::{SegmentedWalWriter, WalError, WalRecord};
use parking_lot::Mutex;

/// Single appender of the log.
pub struct LogWriter {
    /// The segmented log writer.
    writer: Mutex<SegmentedWalWriter>,
    /// End of the durable prefix of the log. Guards the `fsync` so only one runs at a time.
    durable_end: Mutex<Lsn>,
}

/// Positions of one appended transaction.
#[derive(Debug, Clone, Copy)]
pub struct Appended {
    /// Position of the `Commit` record.
    pub commit_lsn: Lsn,
    /// Position right after the transaction.
    pub end: Lsn,
}

impl LogWriter {
    /// Opens the log; everything already in it is durable.
    pub fn open(dir: &Path, segment_bytes: u64) -> Result<Self, WalError> {
        let writer = SegmentedWalWriter::open(dir, segment_bytes)?;
        let end = writer.position()?;
        Ok(Self {
            writer: Mutex::new(writer),
            durable_end: Mutex::new(end),
        })
    }

    /// Appends a transaction's records as one contiguous run (not yet durable).
    pub fn append(&self, records: &[WalRecord]) -> Result<Appended, WalError> {
        let mut writer = self.writer.lock();
        let mut commit_lsn = writer.position()?;
        for record in records {
            commit_lsn = writer.append(record)?;
        }
        Ok(Appended {
            commit_lsn,
            end: writer.position()?,
        })
    }

    /// Returns once every record before `end` is durable, syncing at most once per group.
    ///
    /// The `fsync` runs on a cloned file handle while only the durability lock is held, so
    /// other committers keep appending meanwhile and are covered by the next sync together.
    pub fn sync_through(&self, end: Lsn) -> Result<(), WalError> {
        let mut durable = self.durable_end.lock();
        if *durable >= end {
            return Ok(());
        }
        let (file, position) = self.writer.lock().sync_point()?;
        file.sync_data()?;
        *durable = position;
        Ok(())
    }

    /// End of the log, including records not yet durable.
    pub fn end(&self) -> Result<Lsn, WalError> {
        self.writer.lock().position()
    }

    /// End of the durable prefix; readers of the change feed never go past it.
    pub fn durable_end(&self) -> Lsn {
        *self.durable_end.lock()
    }
}
