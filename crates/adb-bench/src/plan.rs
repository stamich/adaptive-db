//! Measurement plans: how long to warm up and how many samples to take.
//!
//! A *sample* is the mean time of one invocation over a group of `iterations` back-to-back calls,
//! so that a case that takes 20 ns is still measured against a clock that ticks in tens of
//! nanoseconds. Samples are collected in *batches*; the median of a batch is robust against a
//! disturbance that lasts a few samples, and the spread of the batch medians is what the stability
//! verdict of the `stats` module judges.

use std::time::Duration;

/// Environment variable that selects the quick plan (`1`): a smoke run, not release grade.
pub const QUICK_ENV: &str = "ADB_BENCH_QUICK";

/// Environment variable that overrides the minimum duration of one sample, in milliseconds.
pub const MIN_SAMPLE_MS_ENV: &str = "ADB_BENCH_MIN_SAMPLE_MS";

/// How a case is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasurementPlan {
    /// Invocations run and discarded before anything is timed (JIT, caches, page faults).
    pub warmup_invocations: u32,
    /// Calibration doubles the iterations of one sample until the group lasts at least this long.
    pub min_sample: Duration,
    /// Upper bound on the iterations of one sample, so a calibration cannot run away.
    pub max_iterations: u64,
    /// Number of batches of samples.
    pub batches: u32,
    /// Samples per batch.
    pub samples_per_batch: u32,
}

/// Constructors and derived quantities of [`MeasurementPlan`].
impl MeasurementPlan {
    /// The release plan for CPU-bound cases: 5 warm-up calls, samples of at least 50 ms,
    /// 3 batches of 7 samples (the shape ACE uses).
    pub const fn release() -> Self {
        Self {
            warmup_invocations: 5,
            min_sample: Duration::from_millis(50),
            max_iterations: 1 << 20,
            batches: 3,
            samples_per_batch: 7,
        }
    }

    /// A smoke plan: one batch of 5 samples of at least 2 ms. Fast, but not release grade, and the
    /// stability verdict is skipped for it.
    pub const fn quick() -> Self {
        Self {
            warmup_invocations: 1,
            min_sample: Duration::from_millis(2),
            max_iterations: 1 << 20,
            batches: 1,
            samples_per_batch: 5,
        }
    }

    /// The plan for operations that cost milliseconds and wait for the disk (a durable commit):
    /// there is nothing to calibrate, so every sample is one invocation, and the case relies on
    /// more batches (5 of 5) to show its spread.
    pub const fn durable() -> Self {
        Self {
            warmup_invocations: 2,
            min_sample: Duration::ZERO,
            max_iterations: 1,
            batches: 5,
            samples_per_batch: 5,
        }
    }

    /// The durable plan in its quick form: one batch of 3 samples.
    pub const fn durable_quick() -> Self {
        Self {
            warmup_invocations: 1,
            min_sample: Duration::ZERO,
            max_iterations: 1,
            batches: 1,
            samples_per_batch: 3,
        }
    }

    /// Chooses the plan for CPU-bound (`durable == false`) or durable-write cases from the
    /// process environment ([`QUICK_ENV`], [`MIN_SAMPLE_MS_ENV`]).
    pub fn from_env(durable: bool) -> Self {
        Self::from_lookup(durable, |name| std::env::var(name).ok())
    }

    /// Like [`Self::from_env`] with an injectable variable lookup, so tests need not touch the
    /// real environment.
    pub fn from_lookup(durable: bool, lookup: impl Fn(&str) -> Option<String>) -> Self {
        let quick = lookup(QUICK_ENV).is_some_and(|v| v == "1");
        let mut plan = match (durable, quick) {
            (false, false) => Self::release(),
            (false, true) => Self::quick(),
            (true, false) => Self::durable(),
            (true, true) => Self::durable_quick(),
        };
        if !durable {
            if let Some(ms) = lookup(MIN_SAMPLE_MS_ENV).and_then(|v| v.parse::<u64>().ok()) {
                plan.min_sample = Duration::from_millis(ms);
            }
        }
        plan
    }

    /// Whether this is a quick (smoke) plan, which can never produce a release verdict.
    pub fn is_quick(&self) -> bool {
        self.batches < 3
    }

    /// Samples taken in total (`batches * samples_per_batch`).
    pub fn total_samples(&self) -> u32 {
        self.batches * self.samples_per_batch
    }
}
