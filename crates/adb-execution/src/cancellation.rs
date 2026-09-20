//! Module `cancellation` for crate `adb-execution`.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Represents `CancellationToken` state used by this subsystem.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

/// Implements behavior for `CancellationToken`.
impl CancellationToken {
    /// Implements the `new` operation used by this subsystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Implements the `cancel` operation used by this subsystem.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Implements the `is_cancelled` operation used by this subsystem.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
