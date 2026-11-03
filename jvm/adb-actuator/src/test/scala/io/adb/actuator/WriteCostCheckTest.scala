package io.adb.actuator

import io.adb.advisor.ShapeDelta
import io.adb.telemetry.WriteStats
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the write-cost check. */
final class WriteCostCheckTest:
  private val policy = Policy()
  /** Baseline: shape 1 ran 100 times at 2 ms each; 100 writes at 50 us each. */
  private val baseline = Baseline(0L, Map(1L -> ShapeFigures(1L, 100L, 10_000.0, 2_000_000.0)), Some(WriteFigures(100L, 50_000.0)))
  private def after(writeMeanNanos: Long, writes: Long = 100L, readMeanNanos: Long = 50_000L, reads: Long = 100L) =
    AfterPeriod(6,
      Map(1L -> ShapeDelta(1L, reads, 0L, reads * readMeanNanos, reads, reads * 10L, 1.0)),
      Map(7L -> WriteStats(7L, writes, writes, writes * writeMeanNanos)))

  @Test def aSmallSlowdownIsAcceptable(): Unit =
    check(after(writeMeanNanos = 60_000L)) match
      case WriteCostAcceptable(s) => assertEquals(0.2, s, 1e-9)
      case other => fail(s"expected acceptable, got $other")

  @Test def aBigSlowdownThatTheReadsPayForIsAcceptable(): Unit =
    // Writes 2x slower: 100 * 50 us = 5 ms lost; reads saved 100 * 1.95 ms = 195 ms.
    check(after(writeMeanNanos = 100_000L)) match
      case WriteCostAcceptable(s) => assertEquals(1.0, s, 1e-9)
      case other => fail(s"expected acceptable, got $other")

  @Test def aBigSlowdownThatTheReadsDoNotPayForIsARegression(): Unit =
    // Reads barely faster (1.99 ms each vs 2 ms: 1 ms saved in all) but writes lost 500 ms.
    check(after(writeMeanNanos = 100_000L, writes = 10_000L, readMeanNanos = 1_990_000L)) match
      case WriteCostRegression(s, saved, added) =>
        assertEquals(1.0, s, 1e-9)
        assertEquals(1_000_000.0, saved, 1.0)
        assertEquals(10_000 * 50_000.0, added, 1.0)
      case other => fail(s"expected regression, got $other")

  @Test def tooFewWritesAfterwardsMeansUnmeasured(): Unit =
    assertEquals(WriteCostUnmeasured, check(after(writeMeanNanos = 500_000L, writes = 49L)))

  @Test def noWriteBaselineMeansUnmeasured(): Unit =
    assertEquals(WriteCostUnmeasured, WriteCostCheck.check(baseline.copy(writes = None), after(500_000L), 7L, policy))

  @Test def anEntityNobodyWroteToIsUnmeasured(): Unit =
    assertEquals(WriteCostUnmeasured, WriteCostCheck.check(baseline, after(500_000L), 99L, policy))

  @Test def fasterWritesAreAcceptableWithANegativeSlowdown(): Unit =
    check(after(writeMeanNanos = 25_000L)) match
      case WriteCostAcceptable(s) => assertEquals(-0.5, s, 1e-9)
      case other => fail(s"expected acceptable, got $other")

  private def check(a: AfterPeriod) = WriteCostCheck.check(baseline, a, 7L, policy)
