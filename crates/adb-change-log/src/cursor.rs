//! Streaming reads of the change log.
//!
//! A [`ChangeCursor`] remembers the last commit its consumer processed and hands out the next
//! commits in bounded batches. It never loads the log into memory: each call reads only what
//! fits in the batch limits, so a consumer that fell far behind catches up in steady memory.
use adb_core::CommitSeq;

use crate::{error::ChangeLogError, log::ChangeLog, model::ChangeEnvelope};

/// A page of consecutive commits.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeBatch {
    /// Sequence number of the first commit in the batch.
    pub first_commit_seq: CommitSeq,
    /// Sequence number of the last commit in the batch; pass it to resume after the batch.
    pub last_commit_seq: CommitSeq,
    /// Encoded size of the batch, the quantity the byte limit applies to.
    pub bytes: u64,
    /// The commits, ascending; never empty.
    pub commits: Vec<ChangeEnvelope>,
}

/// Reads the change log forward from a position.
#[derive(Debug, Clone)]
pub struct ChangeCursor {
    /// The log being read.
    log: ChangeLog,
    /// Last commit already delivered (the consumer's frontier); the next batch starts above it.
    after: CommitSeq,
}

impl ChangeLog {
    /// Opens a cursor that delivers the commits *above* `after`.
    ///
    /// Fails with [`ChangeLogError::Truncated`] when commits the consumer still needs have been
    /// pruned, i.e. when the oldest stored commit is more than one above `after`.
    pub fn cursor(&self, after: CommitSeq) -> Result<ChangeCursor, ChangeLogError> {
        let cursor = ChangeCursor {
            log: self.clone(),
            after,
        };
        cursor.check_available()?;
        Ok(cursor)
    }
}

impl ChangeCursor {
    /// The last commit delivered so far (the starting point before the first batch).
    pub fn position(&self) -> CommitSeq {
        self.after
    }

    /// Moves the position back to `seq` (never forward), so commits above it are delivered
    /// again. A caller that had to cut a page short (for a size limit of its own) uses this to
    /// resume right after the last commit it actually passed on.
    pub fn rewind_to(&mut self, seq: CommitSeq) {
        if seq < self.after {
            self.after = seq;
        }
    }

    /// Verifies the commit right above the position has not been pruned.
    fn check_available(&self) -> Result<(), ChangeLogError> {
        match self.log.earliest() {
            Some(earliest) if earliest.0 > self.after.0.saturating_add(1) => {
                Err(ChangeLogError::Truncated {
                    requested: self.after.0 + 1,
                    earliest: earliest.0,
                })
            }
            _ => Ok(()),
        }
    }

    /// Returns the next batch, or `None` when the consumer is caught up.
    ///
    /// The batch holds at most `max_commits` commits and stops once `max_bytes` of encoded data
    /// were collected, except that it always holds at least one commit when one is available.
    /// The cursor advances past the returned commits only on success.
    pub fn next_batch(
        &mut self,
        max_commits: usize,
        max_bytes: u64,
    ) -> Result<Option<ChangeBatch>, ChangeLogError> {
        self.check_available()?;
        let (commits, bytes) = self
            .log
            .read_budgeted(self.after, max_commits.max(1), max_bytes)?;
        let (Some(first), Some(last)) = (commits.first(), commits.last()) else {
            return Ok(None);
        };
        let batch = ChangeBatch {
            first_commit_seq: first.commit_seq,
            last_commit_seq: last.commit_seq,
            bytes,
            commits,
        };
        self.after = batch.last_commit_seq;
        Ok(Some(batch))
    }
}
