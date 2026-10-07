//! Per-query execution settings.
use std::{sync::Arc, time::Instant};

use adb_core::CommitTs;

use crate::{limits::ExecutionLimits, memory::MemoryTracker, CancellationToken, ExecutionError};

/// Rows per batch (and per scan page) unless configured otherwise.
pub const DEFAULT_BATCH_SIZE: usize = 1024;

/// Snapshot, batch size, limits, memory budget and cancellation of one query.
///
/// Clones share the cancellation flag and the memory tracker: they describe the same query.
#[derive(Clone)]
pub struct ExecutionContext {
    /// Snapshot every read of the query observes.
    pub snapshot_ts: CommitTs,
    /// Rows per batch and per scan page.
    pub batch_size: usize,
    /// Upper bound on the estimated heap size of one output batch.
    pub memory_limit_bytes: usize,
    /// Point in time after which the query fails with `DeadlineExceeded`.
    pub deadline: Option<Instant>,
    /// Cooperative cancellation flag.
    pub cancellation: CancellationToken,
    /// Runtime limits (change them with [`ExecutionContext::with_limits`]).
    pub limits: ExecutionLimits,
    /// Byte budget shared by every blocking operator of the query.
    pub memory: Arc<MemoryTracker>,
}

impl ExecutionContext {
    /// Defaults for a query reading at `snapshot_ts`.
    pub fn new(snapshot_ts: CommitTs) -> Self {
        let limits = ExecutionLimits::default();
        Self {
            snapshot_ts,
            batch_size: DEFAULT_BATCH_SIZE,
            memory_limit_bytes: 64 * 1024 * 1024,
            deadline: None,
            cancellation: CancellationToken::new(),
            limits,
            memory: MemoryTracker::new(limits.query_memory_bytes),
        }
    }

    /// Replaces the runtime limits and creates a memory tracker with the new budget.
    pub fn with_limits(mut self, limits: ExecutionLimits) -> Self {
        self.memory = MemoryTracker::new(limits.query_memory_bytes);
        self.limits = limits;
        self
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
