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
            let mut result = None;
            for _ in 0..iterations {
                result = Some(black_box(f()));
            }
            let elapsed = start.elapsed().as_nanos() as f64;
            // The previous result is dropped here, after the clock stopped: dropping a database
            // or a temporary directory must not be charged to the case.
            last = result.take().expect("a sample has at least one iteration");
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

/// Like [`measure`], for work that must start from a fresh state each time (recovery after a
/// crash, building an index): `setup` runs untimed and returns the state, `run` consumes it and
/// is timed. There is no calibration, so each sample is exactly one `run`.
///
/// The warm-up runs `setup` and `run` `plan.warmup_invocations` times first. The state `run`
/// returns is dropped after the clock stopped.
pub fn measure_with_setup<S, T>(
    plan: &MeasurementPlan,
    mut setup: impl FnMut() -> S,
    mut run: impl FnMut(S) -> T,
) -> Measured<T> {
    let mut last = run(setup());
    for _ in 0..plan.warmup_invocations {
        last = black_box(run(setup()));
    }
    let mut batches_ns = Vec::with_capacity(plan.batches as usize);
    for _ in 0..plan.batches {
        let mut batch = Vec::with_capacity(plan.samples_per_batch as usize);
        for _ in 0..plan.samples_per_batch {
            let state = setup();
            let start = Instant::now();
            let result = black_box(run(state));
            batch.push(start.elapsed().as_nanos() as f64);
            last = result;
        }
        batches_ns.push(batch);
    }
    Measured {
        plan: *plan,
        iterations_per_sample: 1,
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
