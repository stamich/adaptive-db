package io.adb.actuator

import io.adb.advisor.{ShapeDelta, WorkloadSnapshot, WorkloadWindow}
import io.adb.telemetry.WriteStats
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the effectiveness verifier: win, no win, noisy time, thin evidence. */
final class EffectivenessVerifierTest:
  private val policy = Policy()
  private def delta(id: Long, executions: Long, rowsRead: Long, nanos: Long) =
    ShapeDelta(id, executions, 0L, executions * nanos, executions, executions * rowsRead, 1.0)
  private def baseline(rows: Long = 10_000L, nanos: Long = 2_000_000L, executions: Long = 100L) =
    Baseline(0L, Map(1L -> ShapeFigures.of(delta(1L, executions, rows, nanos))), None)
  private def after(rows: Long, nanos: Long, executions: Long = 100L, windows: Int = 6) =
    AfterPeriod(windows, Map(1L -> delta(1L, executions, rows, nanos)), Map.empty)

  @Test def itIsTooEarlyBeforeVerifyWindowsHavePassed(): Unit =
    assertEquals(NotYet("5 of 6 windows"), EffectivenessVerifier.verify(baseline(), after(10L, 100_000L, windows = 5), policy))

  @Test def anIndexThatCutsRowsReadIsKept(): Unit =
    EffectivenessVerifier.verify(baseline(), after(rows = 10L, nanos = 50_000L), policy) match
      case Keep(reduction, timeRatio, shapes) =>
        assertEquals(1000.0, reduction, 1e-9)
        assertEquals(0.025, timeRatio, 1e-9)
        assertEquals(1, shapes)
      case other => fail(s"expected Keep, got $other")

  @Test def anIndexThatDoesNotReduceRowsReadIsUndone(): Unit =
    EffectivenessVerifier.verify(baseline(), after(rows = 9_000L, nanos = 1_000_000L), policy) match
      case Undo(UndoReason.NoRowsReduction, reduction, _) => assertEquals(10_000.0 / 9_000.0, reduction, 1e-9)
      case other => fail(s"expected Undo(NoRowsReduction), got $other")

  @Test def exactlyTheRequiredReductionIsKept(): Unit =
    assertTrue(EffectivenessVerifier.verify(baseline(), after(rows = 5_000L, nanos = 2_000_000L), policy).isInstanceOf[Keep])

  @Test def noisyTimeDoesNotOverturnAGoodRowsResult(): Unit =
    // 30 % slower on a busy machine, but rows read fell to a hundredth: kept.
    assertTrue(EffectivenessVerifier.verify(baseline(), after(rows = 100L, nanos = 2_600_000L), policy).isInstanceOf[Keep])

  @Test def aClearTimeLossVetoesEvenWhenRowsFell(): Unit =
    EffectivenessVerifier.verify(baseline(), after(rows = 100L, nanos = 5_000_000L), policy) match
      case Undo(UndoReason.SlowerDespiteFewerRows, _, ratio) => assertEquals(2.5, ratio, 1e-9)
      case other => fail(s"expected Undo(SlowerDespiteFewerRows), got $other")

  @Test def aShapeWithTooFewRunsAfterwardsGivesNoVerdict(): Unit =
    assertTrue(EffectivenessVerifier.verify(baseline(), after(rows = 10L, nanos = 1L, executions = 49L), policy).isInstanceOf[Inconclusive])

  @Test def aShapeWithTooFewRunsBeforeGivesNoVerdict(): Unit =
    assertTrue(EffectivenessVerifier.verify(baseline(executions = 49L), after(rows = 10L, nanos = 1L), policy).isInstanceOf[Inconclusive])

  @Test def aShapeThatNoLongerRunsGivesNoVerdict(): Unit =
    assertTrue(EffectivenessVerifier.verify(baseline(), AfterPeriod(6, Map.empty, Map.empty), policy).isInstanceOf[Inconclusive])

  @Test def severalShapesAreWeightedByHowOftenTheyRanAfterwards(): Unit =
    val b = Baseline(0L, Map(
      1L -> ShapeFigures(1L, 100L, 10_000.0, 1_000_000.0),
      2L -> ShapeFigures(2L, 100L, 100.0, 100_000.0)), None)
    val a = AfterPeriod(6, Map(
      1L -> delta(1L, 900L, 10L, 50_000L),      // the helped shape dominates afterwards
      2L -> delta(2L, 100L, 100L, 100_000L)), Map.empty)
    EffectivenessVerifier.verify(b, a, policy) match
      case Keep(reduction, _, 2) => assertEquals((900 * 10_000.0 + 100 * 100.0) / (900 * 10.0 + 100 * 100.0), reduction, 1e-9)
      case other => fail(s"expected Keep over two shapes, got $other")

  @Test def aWorkloadThatReadsNothingAfterwardsIsAnInfiniteReduction(): Unit =
    EffectivenessVerifier.verify(baseline(), after(rows = 0L, nanos = 1_000L), policy) match
      case Keep(reduction, _, _) => assertEquals(Double.PositiveInfinity, reduction)
      case other => fail(s"expected Keep, got $other")

  @Test def theAfterPeriodCountsOnlyWindowsThatStartedAfterTheAction(): Unit =
    def window(start: Long, executions: Long) = WorkloadWindow(start, start + 10L, Map(1L -> delta(1L, executions, 10L, 1000L)),
      Map(7L -> WriteStats(7L, 2L, 2L, 200L)))
    val snapshot = WorkloadSnapshot(Vector(window(0L, 100L), window(10L, 100L), window(20L, 100L), window(30L, 100L)), Map.empty)
    val period = AfterPeriod.since(snapshot, 20L)
    assertEquals(2, period.windows)
    assertEquals(200L, period.shapes(1L).executions)
    assertEquals(4L, period.writes(7L).statements)
    assertEquals(1, AfterPeriod.since(snapshot, 21L).windows, "a window already running at the action is not counted")
