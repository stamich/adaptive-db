package io.adb.bench

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the measurement plans and of the robust statistics. */
final class PlanAndStatsTest:
  /** The release plan is 5 warm-ups and 3 batches of 7 samples of at least 50 ms. */
  @Test def releasePlanHasTheAceShape(): Unit =
    val plan = MeasurementPlan.Release
    assertEquals(5, plan.warmupInvocations)
    assertEquals(50_000_000L, plan.minSampleNs)
    assertEquals(21, plan.totalSamples)
    assertFalse(plan.isQuick)

  /** The durable plan takes one invocation per sample; quick plans are marked quick. */
  @Test def durableAndQuickPlans(): Unit =
    assertEquals(1L, MeasurementPlan.Durable.maxIterations)
    assertEquals(0L, MeasurementPlan.Durable.minSampleNs)
    assertEquals(25, MeasurementPlan.Durable.totalSamples)
    assertTrue(MeasurementPlan.Quick.isQuick)
    assertTrue(MeasurementPlan.DurableQuick.isQuick)

  /** The environment chooses the plan; the minimum sample applies to CPU-bound plans only. */
  @Test def environmentChoosesThePlan(): Unit =
    val none: String => Option[String] = _ => None
    assertEquals(MeasurementPlan.Release, MeasurementPlan.fromLookup(false, none))
    assertEquals(MeasurementPlan.Durable, MeasurementPlan.fromLookup(true, none))
    val quick: String => Option[String] = name => Option.when(name == MeasurementPlan.QuickEnv)("1")
    assertEquals(MeasurementPlan.Quick, MeasurementPlan.fromLookup(false, quick))
    assertEquals(MeasurementPlan.DurableQuick, MeasurementPlan.fromLookup(true, quick))
    val fast: String => Option[String] = name => Option.when(name == MeasurementPlan.MinSampleMsEnv)("10")
    assertEquals(10_000_000L, MeasurementPlan.fromLookup(false, fast).minSampleNs)
    assertEquals(0L, MeasurementPlan.fromLookup(true, fast).minSampleNs)

  /** The headline is the median of the batch medians, not of all samples. */
  @Test def headlineIsTheMedianOfMedians(): Unit =
    val stats = SampleStats.fromBatches(Seq(Seq(1.0, 2.0, 3.0), Seq(10.0, 11.0, 12.0), Seq(4.0, 5.0, 6.0)), 1, quick = false)
    assertEquals(Vector(2.0, 11.0, 5.0), stats.batchMediansNs)
    assertEquals(5.0, stats.medianNs, 1e-12)
    assertEquals(5.0, stats.globalMedianNs, 1e-12)
    assertEquals(1.0, stats.minNs, 1e-12)
    assertEquals(12.0, stats.maxNs, 1e-12)

  /** The batch MAD, in percent of the headline, drives the stability verdict. */
  @Test def batchMadDecidesStability(): Unit =
    val steady = SampleStats.fromBatches(Seq(Seq(100.0, 100.0), Seq(101.0, 101.0), Seq(99.0, 99.0)), 1, quick = false)
    assertEquals(1.0, steady.batchMadPercent, 1e-9)
    assertEquals(Stability.Stable, steady.stability(5.0))
    val noisy = SampleStats.fromBatches(Seq(Seq(100.0), Seq(200.0), Seq(300.0)), 1, quick = false)
    assertEquals(50.0, noisy.batchMadPercent, 1e-9)
    assertEquals(Stability.Unstable, noisy.stability(5.0))
    val quick = SampleStats.fromBatches(Seq(Seq(100.0), Seq(300.0)), 1, quick = true)
    assertEquals(Stability.Skipped, quick.stability(5.0))

  /** Outliers are reported by index and never removed from the statistics. */
  @Test def outliersAreReportedNotRemoved(): Unit =
    // a spread of ordinary samples (so the MAD is not zero) and one that is 50 times slower
    val samples = (1 to 24).map(i => 100.0 + (i % 5)) ++ Seq(5000.0)
    val stats = SampleStats.fromBatches(Seq(samples), 1, quick = false)
    assertEquals(Vector(24), stats.outlierIndices)
    assertEquals(5000.0, stats.maxNs, 1e-12)
    assertEquals(25, stats.samplesNs.size)

  /** A constant series has no MAD, so no sample is called an outlier. */
  @Test def aConstantSeriesHasNoOutliers(): Unit =
    val stats = SampleStats.fromBatches(Seq(Seq(7.0, 7.0, 7.0)), 1, quick = false)
    assertEquals(0.0, stats.madNs, 0.0)
    assertTrue(stats.outlierIndices.isEmpty)
    assertEquals(0.0, stats.cvPercent, 0.0)

  /** Percentiles use the nearest rank. */
  @Test def percentilesAreNearestRank(): Unit =
    val stats = SampleStats.fromBatches(Seq((1 to 100).map(_.toDouble)), 1, quick = false)
    assertEquals(95.0, stats.p95Ns, 0.0)
    assertEquals(99.0, stats.p99Ns, 0.0)
    assertEquals(0.0, SampleStats.median(Seq.empty), 0.0)
    assertEquals(2.5, SampleStats.median(Seq(4.0, 1.0, 2.0, 3.0)), 0.0)

  /** A case without samples is a programming error. */
  @Test def noSamplesIsRejected(): Unit =
    val failed = scala.util.Try(SampleStats.fromBatches(Seq.empty, 1, quick = false)).isFailure
    assertTrue(failed)
