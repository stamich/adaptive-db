//! A fixed durable-write probe, used as a control.
//!
//! A durable-write benchmark is dominated by `fsync`, so its result moves when the disk, the
//! filesystem or the power state changes even if the engine does not. The probe measures that
//! machine part alone: it overwrites one 4 KiB block of a file and calls `fdatasync`, `samples`
//! times. The harness runs it before and after every durable family and reports it next to the
//! family's result. Then a slower disk shows up in the probe and a slower engine does not.
//!
//! The probe is a guide, not a correction. A database commit may need more than one `fsync`, so
//! its time is not a constant multiple of the probe's; the report shows both numbers.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::stats::median;

/// Size of the block the probe writes.
pub const PROBE_BLOCK_BYTES: usize = 4096;

/// Name of the temporary file the probe creates (and removes) in the data directory.
const PROBE_FILE: &str = ".adb-bench-fsync-probe";

/// Writes before the timing starts, so the file exists and the first block is allocated.
const PROBE_WARMUP: usize = 3;

/// The result of one probe run.
#[derive(Debug, Clone, PartialEq)]
pub struct FsyncProbe {
    /// Bytes written per sample.
    pub block_bytes: usize,
    /// Time of each `write` + `fdatasync`, in nanoseconds, in measurement order.
    pub samples_ns: Vec<f64>,
}

/// Inherent methods of [`FsyncProbe`].
impl FsyncProbe {
    /// Runs the probe in `dir` with `samples` timed writes.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the file cannot be created, written or synced.
    pub fn run(dir: &Path, samples: usize) -> std::io::Result<Self> {
        let path = dir.join(PROBE_FILE);
        let outcome = Self::run_on(&path, samples);
        let _ = fs::remove_file(&path);
        outcome
    }

    /// The measurement itself, on a given file.
    fn run_on(path: &Path, samples: usize) -> std::io::Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let block = vec![0xA5u8; PROBE_BLOCK_BYTES];
        let mut write_and_sync = || -> std::io::Result<f64> {
            file.seek(SeekFrom::Start(0))?;
            let start = Instant::now();
            file.write_all(&block)?;
            file.sync_data()?;
            Ok(start.elapsed().as_nanos() as f64)
        };
        for _ in 0..PROBE_WARMUP {
            write_and_sync()?;
        }
        let mut samples_ns = Vec::with_capacity(samples);
        for _ in 0..samples {
            samples_ns.push(write_and_sync()?);
        }
        Ok(Self {
            block_bytes: PROBE_BLOCK_BYTES,
            samples_ns,
        })
    }

    /// Median time of one write + sync in nanoseconds (`0.0` without samples).
    pub fn p50_ns(&self) -> f64 {
        median(&self.samples_ns)
    }

    /// 95th percentile (nearest rank) in nanoseconds (`0.0` without samples).
    pub fn p95_ns(&self) -> f64 {
        if self.samples_ns.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples_ns.clone();
        sorted.sort_by(f64::total_cmp);
        let rank = (0.95 * sorted.len() as f64).ceil() as usize;
        sorted[rank.clamp(1, sorted.len()) - 1]
    }

    /// How far the median of `after` is from the median of `self`, in percent of `self`. A large
    /// value means the machine changed during the family that ran between the two probes.
    pub fn drift_percent(&self, after: &FsyncProbe) -> f64 {
        let before = self.p50_ns();
        if before > 0.0 {
            (after.p50_ns() - before).abs() / before * 100.0
        } else {
            0.0
        }
    }

    /// The probe as a JSON object (times in microseconds).
    pub fn to_json(&self) -> Value {
        json!({
            "block_bytes": self.block_bytes,
            "samples": self.samples_ns.len(),
            "p50_us": self.p50_ns() / 1000.0,
            "p95_us": self.p95_ns() / 1000.0,
        })
    }
}
