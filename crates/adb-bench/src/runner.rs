//! The measurement loop.

use std::hint::black_box;
use std::time::Instant;

use crate::plan::MeasurementPlan;

/// The raw outcome of [`measure`]: every sample, kept in measurement order.
#[derive(Debug, Clone)]
pub struct Measured<T> {
    /// The plan that was used.
    pub plan: MeasurementPlan,
    /// Invocations folded into one sample by the calibration.
    pub iterations_per_sample: u64,
    /// Per batch, the samples in nanoseconds per invocation, in measurement order.
    pub batches_ns: Vec<Vec<f64>>,
    /// The result of the last invocation, so a caller can verify what was measured.
    pub last: T,
}

/// Warms `f` up, calibrates the iterations of one sample, then takes `plan.batches` batches of
/// `plan.samples_per_batch` samples.
///
/// Every result goes through [`black_box`], so the optimiser cannot delete the work. The calls of
/// the calibration are timed but not kept as samples.
pub fn measure<T>(plan: &MeasurementPlan, mut f: impl FnMut() -> T) -> Measured<T> {
    let mut last = f();
    for _ in 0..plan.warmup_invocations {
        last = black_box(f());
    }
    let iterations = calibrate(plan, &mut f, &mut last);
    let mut batches_ns = Vec::with_capacity(plan.batches as usize);
    for _ in 0..plan.batches {
        let mut batch = Vec::with_capacity(plan.samples_per_batch as usize);
        for _ in 0..plan.samples_per_batch {
            let start = Instant::now();
            for _ in 0..iterations {
                last = black_box(f());
            }
            let elapsed = start.elapsed().as_nanos() as f64;
            batch.push(elapsed / iterations as f64);
        }
        batches_ns.push(batch);
    }
    Measured {
        plan: *plan,
        iterations_per_sample: iterations,
        batches_ns,
        last,
    }
}

/// Doubles the iterations of one sample until a group lasts at least `plan.min_sample`, or the
/// cap is reached. With a zero minimum (the durable plan) the answer is 1.
fn calibrate<T>(plan: &MeasurementPlan, f: &mut impl FnMut() -> T, last: &mut T) -> u64 {
    let mut iterations = 1u64;
    loop {
        let start = Instant::now();
        for _ in 0..iterations {
            *last = black_box(f());
        }
        if start.elapsed() >= plan.min_sample || iterations >= plan.max_iterations {
            return iterations;
        }
        iterations = (iterations * 2).min(plan.max_iterations);
    }
}
