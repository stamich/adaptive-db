//! Query-scoped memory accounting.
//!
//! Blocking operators (hash-join build sides, sort buffers, aggregate groups) reserve the bytes
//! they hold from the query's [`MemoryTracker`] *before* holding them. A reservation is an RAII
//! guard: dropping it (or the operator owning it) returns the bytes, so a failed or cancelled
//! query can never leak budget, and two operators of one query share one budget.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::ExecutionError;

/// Atomic byte budget of one query.
#[derive(Debug)]
pub struct MemoryTracker {
    /// Maximum bytes reserved at the same time.
    limit: usize,
    /// Bytes currently reserved.
    used: AtomicUsize,
    /// Highest value `used` has reached (reported by the query profile).
    peak: AtomicUsize,
}

impl MemoryTracker {
    /// A tracker allowing at most `limit` bytes at a time.
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }

    /// An empty reservation that can later [`MemoryReservation::grow`].
    pub fn reservation(self: &Arc<Self>, owner: &'static str) -> MemoryReservation {
        MemoryReservation {
            tracker: Arc::clone(self),
            owner,
            bytes: 0,
        }
    }

    /// Reserves `bytes` atomically, failing without side effects if the budget would be exceeded.
    fn acquire(&self, bytes: usize, owner: &str) -> Result<(), ExecutionError> {
        let mut current = self.used.load(Ordering::Relaxed);
        loop {
            let next = current
                .checked_add(bytes)
                .filter(|next| *next <= self.limit)
                .ok_or_else(|| {
                    ExecutionError::ResourceLimit(format!(
                        "{owner} needs {bytes} more bytes; query memory limit is {} bytes ({current} in use)",
                        self.limit
                    ))
                })?;
            match self.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.peak.fetch_max(next, Ordering::Relaxed);
                    return Ok(());
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// Returns `bytes` to the budget.
    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// Bytes currently reserved.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }

    /// Highest number of bytes reserved at once so far.
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Acquire)
    }

    /// The configured budget.
    pub fn limit(&self) -> usize {
        self.limit
    }
}

/// Bytes held by one operator; released when dropped.
#[derive(Debug)]
pub struct MemoryReservation {
    /// Budget the bytes are reserved from.
    tracker: Arc<MemoryTracker>,
    /// Operator name used in limit errors.
    owner: &'static str,
    /// Bytes currently held.
    bytes: usize,
}

impl MemoryReservation {
    /// Reserves `bytes` more.
    pub fn grow(&mut self, bytes: usize) -> Result<(), ExecutionError> {
        self.tracker.acquire(bytes, self.owner)?;
        self.bytes += bytes;
        Ok(())
    }

    /// Returns up to `bytes` to the budget.
    pub fn shrink(&mut self, bytes: usize) {
        let bytes = bytes.min(self.bytes);
        self.tracker.release(bytes);
        self.bytes -= bytes;
    }

    /// Bytes currently held.
    pub fn size(&self) -> usize {
        self.bytes
    }
}

impl Drop for MemoryReservation {
    /// Returns everything still held.
    fn drop(&mut self) {
        self.tracker.release(self.bytes);
    }
}

/// Unit tests of the tracker and reservation lifecycle.
#[cfg(test)]
mod tests {
    use super::*;

    /// Reservations share one budget and give it back when dropped.
    #[test]
    fn reservations_share_and_release_the_budget() {
        let tracker = MemoryTracker::new(100);
        let mut a = tracker.reservation("a");
        let mut b = tracker.reservation("b");
        a.grow(60).unwrap();
        assert!(b.grow(41).is_err(), "would exceed the shared budget");
        assert_eq!(tracker.used(), 60, "a failed grow reserves nothing");
        b.grow(40).unwrap();
        drop(a);
        assert_eq!(tracker.used(), 40);
        b.shrink(15);
        assert_eq!(tracker.used(), 25);
        drop(b);
        assert_eq!(tracker.used(), 0);
        assert_eq!(tracker.peak(), 100);
    }

    /// Concurrent reservations never exceed the limit.
    #[test]
    fn concurrent_reservations_respect_the_limit() {
        let tracker = MemoryTracker::new(1_000);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let tracker = Arc::clone(&tracker);
                std::thread::spawn(move || {
                    let mut held = Vec::new();
                    for _ in 0..1_000 {
                        let mut reservation = tracker.reservation("t");
                        if reservation.grow(7).is_ok() {
                            held.push(reservation);
                        }
                        if held.len() > 10 {
                            held.clear();
                        }
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(tracker.peak() <= 1_000);
        assert_eq!(tracker.used(), 0);
    }
}
