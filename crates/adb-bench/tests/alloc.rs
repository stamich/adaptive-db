//! Tests of the counting allocator (this test binary installs it).
use adb_bench::alloc::{count, live_bytes, peak_bytes, peak_rss_bytes, reset_peak};
use adb_bench::{AllocUsage, CountingAllocator};
use std::sync::{Mutex, MutexGuard, PoisonError};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// `live_bytes` and `peak_bytes` are process-wide, and the test harness runs tests on parallel
/// threads: a test that frees a megabyte while another compares two readings of `live_bytes` makes
/// that comparison fail. Every test holds this lock.
static SERIAL: Mutex<()> = Mutex::new(());

/// Takes [`SERIAL`], also after another test panicked while holding it.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn a_closure_that_allocates_is_counted() {
    let _serial = serial();
    let (vector, usage) = count(|| vec![0u8; 4096]);
    assert!(usage.bytes >= 4096, "{usage:?}");
    assert!(usage.calls >= 1);
    assert_eq!(vector.len(), 4096);
}

#[test]
fn a_closure_that_does_not_allocate_counts_zero() {
    let _serial = serial();
    let (sum, usage) = count(|| (0..1000u64).sum::<u64>());
    assert_eq!(sum, 499_500);
    assert_eq!(usage, AllocUsage::default());
}

#[test]
fn growing_a_buffer_counts_the_growth() {
    let _serial = serial();
    let (_, usage) = count(|| {
        let mut buffer: Vec<u8> = Vec::with_capacity(16);
        buffer.extend_from_slice(&[1u8; 16]);
        buffer.extend_from_slice(&[1u8; 4096]);
        buffer
    });
    assert!(usage.bytes >= 4096 + 16, "{usage:?}");
}

#[test]
fn other_threads_do_not_leak_into_the_count() {
    let _serial = serial();
    let ((), usage) = count(|| {
        std::thread::spawn(|| drop(vec![0u8; 1 << 20]))
            .join()
            .unwrap();
    });
    assert!(
        usage.bytes < 64 * 1024,
        "the 1 MiB of the other thread was counted: {usage:?}"
    );
}

#[test]
fn live_and_peak_bytes_follow_the_heap() {
    let _serial = serial();
    // The counters are process-wide: the test harness's own threads allocate and free a few
    // hundred bytes at any moment, so the comparisons allow for that much.
    const BLOCK: u64 = 8 << 20;
    const SLACK: u64 = 64 << 10;
    reset_peak();
    let before = live_bytes();
    let block = vec![0u8; BLOCK as usize];
    let after = live_bytes();
    assert!(
        after + SLACK >= before + BLOCK,
        "before {before} after {after}"
    );
    assert!(peak_bytes() + SLACK >= before + BLOCK);
    drop(block);
    assert!(live_bytes() < before + BLOCK - SLACK);
    assert!(
        peak_bytes() + SLACK >= before + BLOCK,
        "the peak does not fall when memory is freed"
    );
}

#[test]
fn per_run_divides_and_survives_zero() {
    let _serial = serial();
    let usage = AllocUsage {
        bytes: 1000,
        calls: 10,
    };
    assert_eq!(usage.per_run(10), (100.0, 1.0));
    assert_eq!(usage.per_run(0), (0.0, 0.0));
}

#[test]
fn the_peak_rss_is_readable_on_linux() {
    let _serial = serial();
    if cfg!(target_os = "linux") {
        assert!(peak_rss_bytes().unwrap() > 0);
    }
}
