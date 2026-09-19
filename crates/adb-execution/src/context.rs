//! Module `context` for crate `adb-execution`.
use std::time::Instant;

use adb_core::CommitTs;

use crate::{CancellationToken, ExecutionError};

/// Defines the `DEFAULT_BATCH_SIZE` constant used by this subsystem.
pub const DEFAULT_BATCH_SIZE: usize = 1024;

/// Represents `ExecutionContext` state used by this subsystem.
#[derive(Clone)]
pub struct ExecutionContext {
    pub snapshot_ts: CommitTs,
    pub batch_size: usize,
    pub memory_limit_bytes: usize,
    pub deadline: Option<Instant>,
    pub cancellation: CancellationToken,
}

/// Implements behavior for `ExecutionContext`.
impl ExecutionContext {
    /// Implements the `new` operation used by this subsystem.
    pub fn new(snapshot_ts: CommitTs) -> Self {
        Self {
            snapshot_ts,
            batch_size: DEFAULT_BATCH_SIZE,
            memory_limit_bytes: 64 * 1024 * 1024,
            deadline: None,
            cancellation: CancellationToken::new(),
        }
    }

    /// Implements the `check_running` operation used by this subsystem.
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
