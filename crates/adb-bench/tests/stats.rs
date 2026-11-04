//! Tests of the statistics, on sample sets whose answers are known by hand.
use adb_bench::stats::{median, DEFAULT_STABLE_BATCH_MAD_PERCENT};
use adb_bench::{SampleStats, Stability};

fn stats(batches: &[Vec<f64>], quick: bool) -> SampleStats {
    SampleStats::from_batches(batches, 1, quick)
}

#[test]
fn median_handles_odd_even_and_empty() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
    assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
    assert_eq!(median(&[]), 0.0);
    assert_eq!(median(&[7.0]), 7.0);
}

#[test]
fn median_of_medians_ignores_a_disturbed_batch() {
    let s = stats(&[vec![10.0; 7], vec![10.0; 7], vec![500.0; 7]], false);
    assert_eq!(s.batch_medians_ns, vec![10.0, 10.0, 500.0]);
    assert_eq!(
        s.median_ns, 10.0,
        "one bad batch cannot move the headline value"
    );
    // ...but the verdict sees the disagreement only if it reaches the MAD: here two of three agree.
    assert_eq!(s.batch_mad_percent, 0.0);
}

#[test]
fn batch_mad_percent_measures_disagreement_between_batches() {
    // Batch medians 11, 21, 31: MoM 21, deviations 10, 0, 10, MAD 10 -> 47.6 %.
    let s = stats(
        &[
            vec![10.0, 11.0, 12.0],
            vec![20.0, 21.0, 22.0],
            vec![30.0, 31.0, 32.0],
        ],
        false,
    );
    assert_eq!(s.median_ns, 21.0);
    assert!((s.batch_mad_percent - 10.0 / 21.0 * 100.0).abs() < 1e-9);
    assert_eq!(
        s.stability(DEFAULT_STABLE_BATCH_MAD_PERCENT),
        Stability::Unstable
    );
    assert_eq!(s.stability(50.0), Stability::Stable);
}

#[test]
fn identical_batches_are_stable() {
    let s = stats(&[vec![5.0, 5.0], vec![5.0, 5.0], vec![5.0, 5.0]], false);
    assert_eq!(s.batch_mad_percent, 0.0);
    assert_eq!(
        s.stability(DEFAULT_STABLE_BATCH_MAD_PERCENT),
        Stability::Stable
    );
    assert_eq!(s.stddev_ns, 0.0);
    assert_eq!(s.cv_percent, 0.0);
}

#[test]
fn a_quick_plan_never_gets_a_verdict() {
    let s = stats(&[vec![1.0, 100.0, 1.0, 100.0, 1.0]], true);
    assert_eq!(s.stability(0.0), Stability::Skipped);
    assert_eq!(Stability::Skipped.as_str(), "skipped");
    assert_eq!(Stability::Stable.as_str(), "stable");
    assert_eq!(Stability::Unstable.as_str(), "unstable");
}

#[test]
fn outliers_are_reported_by_index_and_kept() {
    let mut batch = vec![10.0, 11.0, 9.0, 10.0, 10.0, 11.0, 9.0, 10.0];
    batch.push(1000.0);
    let s = stats(&[batch.clone()], false);
    assert_eq!(s.outlier_indices, vec![8]);
    assert_eq!(
        s.samples_ns.len(),
        9,
        "an outlier is reported, never removed"
    );
    assert_eq!(s.max_ns, 1000.0);
    assert_eq!(s.min_ns, 9.0);
}

#[test]
fn a_constant_series_has_no_outliers() {
    let s = stats(&[vec![4.0; 10]], false);
    assert!(s.outlier_indices.is_empty());
    assert_eq!(s.mad_ns, 0.0);
}

#[test]
fn percentiles_use_the_nearest_rank() {
    let batch: Vec<f64> = (1..=100).map(f64::from).collect();
    let s = stats(&[batch], false);
    assert_eq!(s.p95_ns, 95.0);
    assert_eq!(s.p99_ns, 99.0);
    assert_eq!(s.mean_ns, 50.5);
    assert_eq!(s.global_median_ns, 50.5);
}

#[test]
fn the_shape_of_the_plan_is_recorded() {
    let s = SampleStats::from_batches(&[vec![1.0, 2.0, 3.0], vec![1.0, 2.0, 3.0]], 64, false);
    assert_eq!(s.samples_per_batch, 3);
    assert_eq!(s.iterations_per_sample, 64);
    assert_eq!(s.samples_ns.len(), 6);
}

#[test]
#[should_panic(expected = "at least one sample")]
fn no_samples_is_a_bug() {
    let _ = stats(&[], false);
}
