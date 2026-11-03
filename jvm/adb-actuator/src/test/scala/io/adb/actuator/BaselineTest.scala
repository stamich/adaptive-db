package io.adb.actuator

import io.adb.advisor.{ShapeDelta, WorkloadSnapshot}
import io.adb.telemetry.WriteStats
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of baseline capture. */
final class BaselineTest:
  private def delta(id: Long, executions: Long, rowsRead: Long, nanos: Long) =
    ShapeDelta(id, executions, 0L, nanos, executions, rowsRead, 1.0)
  private val workload = WorkloadSnapshot(Vector.empty,
    Map(1L -> delta(1L, 100L, 100L * 5000L, 100L * 2_000_000L), 2L -> delta(2L, 10L, 10L * 40L, 10L * 100_000L), 3L -> delta(3L, 500L, 500L, 500L)),
    Map(7L -> WriteStats(7L, 40L, 40L, 40L * 50_000L)))

  @Test def theBaselineHoldsPerExecutionFiguresOfTheEvidenceShapesOnly(): Unit =
    val b = BaselineCapture.take(Vector(1L, 2L), 7L, workload, 123L)
    assertEquals(123L, b.takenAtNanos)
    assertEquals(Set(1L, 2L), b.shapes.keySet, "shape 3 did not justify the action")
    assertEquals(ShapeFigures(1L, 100L, 5000.0, 2_000_000.0), b.shapes(1L))

  @Test def aShapeThatLeftTheWindowsIsSimplyAbsent(): Unit =
    assertEquals(Set(1L), BaselineCapture.take(Vector(1L, 99L), 7L, workload, 0L).shapes.keySet)

  @Test def writeFiguresComeFromTheEntityAndAreAbsentWithoutWrites(): Unit =
    assertEquals(Some(WriteFigures(40L, 50_000.0)), BaselineCapture.take(Vector(1L), 7L, workload, 0L).writes)
    assertEquals(None, BaselineCapture.take(Vector(1L), 8L, workload, 0L).writes)

  @Test def onlyShapesWithEnoughExecutionsAreUsable(): Unit =
    val b = BaselineCapture.take(Vector(1L, 2L), 7L, workload, 0L)
    assertEquals(Set(1L), b.usableShapes(50L).keySet)
    assertEquals(Set(1L, 2L), b.usableShapes(10L).keySet)
