package io.adb.advisor

import io.adb.telemetry.ShapeStats
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the windowed workload model. */
final class WorkloadModelTest:
  /** Cumulative counters of shape `id` after `n` executions of 1 ms, 100 rows read and 2 returned each. */
  private def stats(id: Long, n: Long, qError: Double = 1.0): ShapeStats =
    ShapeStats(id, n, 0L, 0L, n * 1_000_000L, 1_000_000L, n * 2, n * 100, qError)

  @Test def theFirstObservationOnlySetsTheBaseline(): Unit =
    val model = new WorkloadModel(4)
    val first = model.observe(Vector(stats(1, 500)), 0L)
    assertTrue(first.shapes.isEmpty, "counters since process start are not one interval")
    assertTrue(model.snapshot().shapes.isEmpty)

  @Test def windowsHoldDeltasNotCumulativeCounters(): Unit =
    val model = new WorkloadModel(4)
    model.observe(Vector(stats(1, 500)), 0L)
    val w = model.observe(Vector(stats(1, 530)), 1_000L)
    assertEquals(30L, w.shapes(1L).executions)
    assertEquals(3000L, w.shapes(1L).totalRowsRead)
    assertEquals(60L, w.shapes(1L).totalActualRows)
    assertEquals(1_000L, w.spanNanos)

  @Test def idleShapesAreAbsentFromAWindow(): Unit =
    val model = new WorkloadModel(4)
    model.observe(Vector(stats(1, 5), stats(2, 5)), 0L)
    val w = model.observe(Vector(stats(1, 9), stats(2, 5)), 10L)
    assertEquals(Set(1L), w.shapes.keySet)

  @Test def oldWindowsAreDroppedAndTotalsFollow(): Unit =
    val model = new WorkloadModel(2)
    model.observe(Vector(stats(1, 0)), 0L)
    model.observe(Vector(stats(1, 10)), 10L)
    model.observe(Vector(stats(1, 30)), 20L)
    model.observe(Vector(stats(1, 60)), 30L)
    val snap = model.snapshot()
    assertEquals(2, snap.windows.size)
    assertEquals(50L, snap.shapes(1L).executions, "only the last two windows (20 + 30) remain")
    assertEquals(20L, snap.spanNanos)

  @Test def aCounterResetIsReadAsAFreshStart(): Unit =
    val model = new WorkloadModel(4)
    model.observe(Vector(stats(1, 1000)), 0L)
    val w = model.observe(Vector(stats(1, 7)), 5L)
    assertEquals(7L, w.shapes(1L).executions)

  @Test def derivedFiguresAreReadFromTheTotals(): Unit =
    val model = new WorkloadModel(4)
    model.observe(Vector(stats(1, 0), stats(2, 0)), 0L)
    model.observe(Vector(stats(1, 10, 8.0), stats(2, 30)), 10L)
    val snap = model.snapshot()
    val d = snap.shapes(1L)
    assertEquals(100.0, d.meanRowsRead, 1e-9)
    assertEquals(2.0, d.meanActualRows, 1e-9)
    assertEquals(50.0, d.scanAmplification, 1e-9)
    assertEquals(8.0, d.maxQError, 1e-9)
    assertEquals(0.25, snap.timeShare(1L), 1e-9)
    assertEquals(0.0, snap.timeShare(99L), 1e-9)
    assertEquals(40L, snap.totalExecutions)

  @Test def clearForgetsTheBaseline(): Unit =
    val model = new WorkloadModel(4)
    model.observe(Vector(stats(1, 0)), 0L)
    model.observe(Vector(stats(1, 5)), 1L)
    model.clear()
    assertEquals(0, model.windowCount)
    assertTrue(model.observe(Vector(stats(1, 50)), 2L).shapes.isEmpty)

  @Test def emptyWorkloadHasNoShare(): Unit =
    assertEquals(0L, WorkloadSnapshot.empty.spanNanos)
    assertEquals(0.0, WorkloadSnapshot.empty.timeShare(1L), 1e-9)
