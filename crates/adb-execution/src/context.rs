//! Per-query execution settings.
use std::time::Instant;

use adb_core::CommitTs;

use crate::{CancellationToken, ExecutionError};

/// Rows per batch (and per scan page) unless configured otherwise.
pub const DEFAULT_BATCH_SIZE: usize = 1024;

/// Snapshot, batch size, limits and cancellation of one query.
#[derive(Clone)]
pub struct ExecutionContext {
    pub snapshot_ts: CommitTs,
    pub batch_size: usize,
    pub memory_limit_bytes: usize,
    pub deadline: Option<Instant>,
    pub cancellation: CancellationToken,
}

impl ExecutionContext {
    /// Defaults for a query reading at `snapshot_ts`.
    pub fn new(snapshot_ts: CommitTs) -> Self {
        Self {
            snapshot_ts,
            batch_size: DEFAULT_BATCH_SIZE,
            memory_limit_bytes: 64 * 1024 * 1024,
            deadline: None,
            cancellation: CancellationToken::new(),
        }
    }

    /// Fails if the query was cancelled or ran past its deadline.
    pub fn check_running(&self) -> Result<(), ExecutionError> {
        if self.cancellation.is_cancelled() {
            return Err(ExecutionError::Cancelled);
        }

        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ExecutionError::DeadlineExceeded);
        }

        Ok(())
    }
}
