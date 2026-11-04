//! A counting global allocator, for the `memory` family.
//!
//! Time depends on the machine; the number of bytes a code path allocates does not. A benchmark
//! binary that wants those numbers installs [`CountingAllocator`] as its `#[global_allocator]`;
//! [`count`] then reports what a closure allocated **on the calling thread**. Counters are
//! per thread (a `const`-initialised, destructor-free thread local, so reading them never
//! allocates), which keeps one thread's figures free of another's. A process-wide live and peak
//! byte count is kept as well, for the heap high-water mark of a whole run.
//!
//! The allocator adds a few instructions to every allocation, so timing benchmarks must **not**
//! install it; the memory benchmark is a separate binary for that reason.

// This module is the one place of the crate that needs `unsafe`: `GlobalAlloc` is an unsafe trait.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    /// Bytes requested from the allocator by this thread, since it started.
    static THREAD_BYTES: Cell<u64> = const { Cell::new(0) };
    /// Allocation calls (`alloc`, `alloc_zeroed` and growing `realloc`) of this thread.
    static THREAD_CALLS: Cell<u64> = const { Cell::new(0) };
}

/// Bytes currently allocated in the whole process.
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

/// Highest value [`LIVE_BYTES`] has had since the last [`reset_peak`].
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);

/// What a closure allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllocUsage {
    /// Bytes requested from the allocator (a `realloc` counts the growth only).
    pub bytes: u64,
    /// Number of allocation calls.
    pub calls: u64,
}

/// Arithmetic on [`AllocUsage`].
impl AllocUsage {
    /// This usage divided by `n` runs, as `(bytes per run, calls per run)`. Zero for `n == 0`.
    pub fn per_run(self, n: u64) -> (f64, f64) {
        if n == 0 {
            (0.0, 0.0)
        } else {
            (self.bytes as f64 / n as f64, self.calls as f64 / n as f64)
        }
    }
}

/// Wraps the system allocator and counts what it hands out. Install with
/// `#[global_allocator] static ALLOC: CountingAllocator = CountingAllocator;`.
pub struct CountingAllocator;

/// Records `bytes` for the calling thread and the process.
fn record(bytes: u64) {
    THREAD_BYTES.with(|b| b.set(b.get() + bytes));
    THREAD_CALLS.with(|c| c.set(c.get() + 1));
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every method forwards to `System` with the same arguments and only adds counting that
// neither allocates nor panics, so the contract of `GlobalAlloc` is `System`'s.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            record(layout.size() as u64);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc_zeroed(layout);
        if !pointer.is_null() {
            record(layout.size() as u64);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = System.realloc(pointer, layout, new_size);
        if !new_pointer.is_null() {
            if new_size > layout.size() {
                record((new_size - layout.size()) as u64);
            } else {
                LIVE_BYTES.fetch_sub((layout.size() - new_size) as u64, Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

/// Runs `f` and returns its result with what the calling thread allocated meanwhile. Zero if
/// [`CountingAllocator`] is not the global allocator.
pub fn count<T>(f: impl FnOnce() -> T) -> (T, AllocUsage) {
    let bytes = THREAD_BYTES.with(Cell::get);
    let calls = THREAD_CALLS.with(Cell::get);
    let result = f();
    let usage = AllocUsage {
        bytes: THREAD_BYTES.with(Cell::get) - bytes,
        calls: THREAD_CALLS.with(Cell::get) - calls,
    };
    (result, usage)
}

/// Bytes currently allocated in the whole process.
pub fn live_bytes() -> u64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Highest number of live bytes since the last [`reset_peak`] (or process start).
pub fn peak_bytes() -> u64 {
    PEAK_BYTES.load(Ordering::Relaxed)
}

/// Sets the peak to the current live bytes, to measure the high-water mark of one phase.
pub fn reset_peak() {
    PEAK_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Peak resident set size of the process in bytes (`VmHWM` of `/proc/self/status`), when readable.
pub fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}
