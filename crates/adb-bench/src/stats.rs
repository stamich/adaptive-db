//! Robust summary statistics of batched timing samples.
//!
//! Definitions (the same as ACE's Harness V3, so the two projects read alike):
//!
//! * **median of medians (MoM)**: the median of the per-batch medians. It is the headline value of a
//!   case: one disturbed batch cannot move it.
//! * **batch MAD %**: the median absolute deviation of the batch medians from the MoM, relative to
//!   the MoM. It is the input of the stability verdict.
//! * **outlier**: a sample farther than `3 * 1.4826 * MAD` from the global median. Outliers are
//!   reported and never removed.

use crate::runner::Measured;

/// Batch-MAD limit (percent) of the per-case verdict when a family sets no limit of its own.
/// Starting value, to be replaced by limits measured on the reference machine (see
/// `docs/BENCHMARKS.md`).
pub const DEFAULT_STABLE_BATCH_MAD_PERCENT: f64 = 5.0;

/// Ratio between the MAD and the standard deviation of a normal distribution.
pub const MAD_SIGMA_SCALE: f64 = 1.4826;

/// Number of scaled MADs beyond which a sample is an outlier.
pub const OUTLIER_SIGMAS: f64 = 3.0;

/// Whether the batch medians of a case agree closely enough to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// The batch MAD is within the limit.
    Stable,
    /// The batch MAD is above the limit; the case must be measured again.
    Unstable,
    /// The plan was quick, so no verdict is given.
    Skipped,
}

/// Inherent methods of [`Stability`].
impl Stability {
    /// The lower-case word used in the JSON reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Stability::Stable => "stable",
            Stability::Unstable => "unstable",
            Stability::Skipped => "skipped",
        }
    }
}

/// Timing samples of one case and their summaries. All times are nanoseconds per invocation.
#[derive(Debug, Clone)]
pub struct SampleStats {
    /// Every sample, batch-major, in measurement order.
    pub samples_ns: Vec<f64>,
    /// The median of each batch, in batch order.
    pub batch_medians_ns: Vec<f64>,
    /// Samples per batch.
    pub samples_per_batch: usize,
    /// Invocations folded into one sample by the calibration.
    pub iterations_per_sample: u64,
    /// Median of the batch medians: the headline value.
    pub median_ns: f64,
    /// Median of all samples (diagnostic).
    pub global_median_ns: f64,
    /// Smallest sample.
    pub min_ns: f64,
    /// Largest sample.
    pub max_ns: f64,
    /// 95th percentile of all samples (nearest rank).
    pub p95_ns: f64,
    /// 99th percentile of all samples (nearest rank).
    pub p99_ns: f64,
    /// Arithmetic mean of all samples.
    pub mean_ns: f64,
    /// Population standard deviation of all samples.
    pub stddev_ns: f64,
    /// Standard deviation over the mean, in percent (diagnostic only).
    pub cv_percent: f64,
    /// Median absolute deviation of all samples from [`Self::global_median_ns`].
    pub mad_ns: f64,
    /// Median absolute deviation of the batch medians from [`Self::median_ns`], in percent of it.
    pub batch_mad_percent: f64,
    /// Indices into [`Self::samples_ns`] of the outliers.
    pub outlier_indices: Vec<usize>,
    /// Whether the plan that produced the samples was quick (no stability verdict then).
    pub quick: bool,
}

/// Constructors and verdicts of [`SampleStats`].
impl SampleStats {
    /// Summarises the output of [`crate::measure`].
    ///
    /// # Panics
    ///
    /// Panics if there is no sample at all, which no [`crate::MeasurementPlan`] produces.
    pub fn from_measured<T>(measured: &Measured<T>) -> Self {
        Self::from_batches(
            &measured.batches_ns,
            measured.iterations_per_sample,
            measured.plan.is_quick(),
        )
    }

    /// Summarises batches of samples (nanoseconds per invocation).
    ///
    /// # Panics
    ///
    /// Panics if `batches` holds no sample.
    pub fn from_batches(batches: &[Vec<f64>], iterations_per_sample: u64, quick: bool) -> Self {
        let samples_ns: Vec<f64> = batches.iter().flatten().copied().collect();
        assert!(!samples_ns.is_empty(), "a case needs at least one sample");
        let batch_medians_ns: Vec<f64> = batches.iter().map(|b| median(b)).collect();
        let median_ns = median(&batch_medians_ns);
        let global_median_ns = median(&samples_ns);
        let mad_ns = median_of(samples_ns.iter().map(|s| (s - global_median_ns).abs()));
        let batch_mad_ns = median_of(batch_medians_ns.iter().map(|m| (m - median_ns).abs()));
        let batch_mad_percent = if median_ns > 0.0 {
            batch_mad_ns / median_ns * 100.0
        } else {
            0.0
        };
        let n = samples_ns.len() as f64;
        let mean_ns = samples_ns.iter().sum::<f64>() / n;
        let variance = samples_ns
            .iter()
            .map(|s| (s - mean_ns).powi(2))
            .sum::<f64>()
            / n;
        let stddev_ns = variance.sqrt();
        let mut sorted = samples_ns.clone();
        sorted.sort_by(f64::total_cmp);
        let threshold = OUTLIER_SIGMAS * MAD_SIGMA_SCALE * mad_ns;
        let outlier_indices = if mad_ns > 0.0 {
            samples_ns
                .iter()
                .enumerate()
                .filter(|(_, s)| (**s - global_median_ns).abs() > threshold)
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        Self {
            samples_per_batch: batches.first().map_or(0, Vec::len),
            iterations_per_sample,
            median_ns,
            global_median_ns,
            min_ns: sorted[0],
            max_ns: sorted[sorted.len() - 1],
            p95_ns: nearest_rank(&sorted, 0.95),
            p99_ns: nearest_rank(&sorted, 0.99),
            mean_ns,
            stddev_ns,
            cv_percent: if mean_ns > 0.0 {
                stddev_ns / mean_ns * 100.0
            } else {
                0.0
            },
            mad_ns,
            batch_mad_percent,
            outlier_indices,
            samples_ns,
            batch_medians_ns,
            quick,
        }
    }

    /// The stability verdict for a batch-MAD `limit_percent`; a quick plan gives
    /// [`Stability::Skipped`].
    pub fn stability(&self, limit_percent: f64) -> Stability {
        if self.quick {
            Stability::Skipped
        } else if self.batch_mad_percent <= limit_percent {
            Stability::Stable
        } else {
            Stability::Unstable
        }
    }
}

/// Median of a slice (the mean of the two middle values for an even count).
pub fn median(values: &[f64]) -> f64 {
    median_of(values.iter().copied())
}

/// Median of an iterator of values. Returns `0.0` for no value.
fn median_of(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    }
}

/// Nearest-rank percentile of an ascending slice: the smallest value that is at least `p` of the
/// samples.
fn nearest_rank(sorted: &[f64], p: f64) -> f64 {
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}
