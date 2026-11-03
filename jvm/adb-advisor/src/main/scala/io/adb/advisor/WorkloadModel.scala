package io.adb.advisor

import io.adb.telemetry.ShapeStats

/** What one query shape did inside a time window (or, summed, inside several windows).
  *
  * Unlike [[ShapeStats]], which is cumulative since the process started, a delta describes only
  * the interval it covers, so a workload that changed an hour ago stops looking hot.
  *
  * @param shapeId         the shape
  * @param executions      captured executions in the interval
  * @param failures        of which failed
  * @param totalNanos      elapsed time of those executions
  * @param totalActualRows rows they returned
  * @param totalRowsRead   rows they scanned
  * @param maxQError       worst estimate error seen in the interval
  */
final case class ShapeDelta(
    shapeId: Long,
    executions: Long,
    failures: Long,
    totalNanos: Long,
    totalActualRows: Long,
    totalRowsRead: Long,
    maxQError: Double
) derives CanEqual:
  /** Mean elapsed nanoseconds per captured execution (0 when there was none). */
  def meanNanos: Double = if executions == 0 then 0.0 else totalNanos.toDouble / executions

  /** Mean rows scanned per execution. */
  def meanRowsRead: Double = if executions == 0 then 0.0 else totalRowsRead.toDouble / executions

  /** Mean rows returned per execution. */
  def meanActualRows: Double = if executions == 0 then 0.0 else totalActualRows.toDouble / executions

  /** Rows scanned per row returned; 1.0 is a perfectly selective read. A shape that returned
    * nothing counts its scanned rows over one so it still shows up.
    */
  def scanAmplification: Double = totalRowsRead.toDouble / math.max(totalActualRows, 1L).toDouble

  /** Adds another interval of the same shape. */
  def plus(other: ShapeDelta): ShapeDelta = ShapeDelta(
    shapeId,
    executions + other.executions,
    failures + other.failures,
    totalNanos + other.totalNanos,
    totalActualRows + other.totalActualRows,
    totalRowsRead + other.totalRowsRead,
    math.max(maxQError, other.maxQError)
  )

/** One time window of the workload model.
  *
  * @param startedAtNanos start of the window, on the clock given to [[WorkloadModel.observe]]
  * @param endedAtNanos   end of the window
  * @param shapes         what each shape did inside it (shapes that were idle are absent)
  */
final case class WorkloadWindow(startedAtNanos: Long, endedAtNanos: Long, shapes: Map[Long, ShapeDelta]) derives CanEqual:
  /** Length of the window in nanoseconds. */
  def spanNanos: Long = endedAtNanos - startedAtNanos

/** An immutable view of the workload over the retained windows. Everything the detectors read.
  *
  * @param windows retained windows, oldest first
  * @param shapes  per-shape totals over all retained windows
  */
final case class WorkloadSnapshot(windows: Vector[WorkloadWindow], shapes: Map[Long, ShapeDelta]) derives CanEqual:
  /** Time covered by the retained windows, in nanoseconds. */
  def spanNanos: Long = if windows.isEmpty then 0L else windows.last.endedAtNanos - windows.head.startedAtNanos

  /** Captured executions of all shapes over the retained windows. */
  def totalExecutions: Long = shapes.valuesIterator.map(_.executions).sum

  /** Elapsed time of all shapes over the retained windows, in nanoseconds. */
  def totalNanos: Long = shapes.valuesIterator.map(_.totalNanos).sum

  /** Share of the workload's elapsed time spent in `shapeId`, in `[0, 1]`. */
  def timeShare(shapeId: Long): Double =
    val total = totalNanos
    if total <= 0L then 0.0 else shapes.get(shapeId).fold(0.0)(_.totalNanos.toDouble / total)

/** Constants of [[WorkloadSnapshot]]. */
object WorkloadSnapshot:
  /** A workload in which nothing ran. */
  val empty: WorkloadSnapshot = WorkloadSnapshot(Vector.empty, Map.empty)

/** The advisor's model of the workload: a bounded ring of time windows built from the cumulative
  * per-shape counters of the telemetry runtime.
  *
  * Each call of [[observe]] turns the counters into a **delta** against the previous call and
  * stores it as one window; the oldest window is dropped once `maxWindows` are kept. The model
  * reads only the immutable [[ShapeStats]] values it is handed, never the telemetry ring, so it
  * cannot slow the query path. It is thread-safe; [[snapshot]] returns an immutable copy.
  *
  * @param maxWindows number of windows kept (at least 1)
  */
final class WorkloadModel(maxWindows: Int):
  require(maxWindows >= 1, "maxWindows must be at least 1")

  /** Cumulative counters seen by the previous [[observe]], by shape id. */
  private var previous: Map[Long, ShapeStats] = Map.empty
  /** Clock value of the previous [[observe]], or `None` before the first. */
  private var lastAt: Option[Long] = None
  /** Retained windows, oldest first. */
  private var windows: Vector[WorkloadWindow] = Vector.empty

  /** Closes the current window with the counters read now.
    *
    * The first call only establishes the baseline: it returns an empty window and counts nothing,
    * because the counters hold everything since the process started and not one interval. A
    * counter that went backwards (telemetry was reset) is read as a fresh start.
    *
    * @param cumulative per-shape counters, as [[io.adb.telemetry.TelemetryRuntime.allStats]] returns them
    * @param nowNanos   monotonic clock reading, in nanoseconds
    * @return the window that was closed
    */
  def observe(cumulative: Vector[ShapeStats], nowNanos: Long): WorkloadWindow = synchronized {
    val window = lastAt match
      case None => WorkloadWindow(nowNanos, nowNanos, Map.empty)
      case Some(start) =>
        val deltas = cumulative.iterator.flatMap(now => delta(previous.get(now.shapeId), now)).map(d => d.shapeId -> d).toMap
        WorkloadWindow(start, math.max(nowNanos, start), deltas)
    previous = cumulative.iterator.map(s => s.shapeId -> s).toMap
    lastAt = Some(nowNanos)
    windows = (windows :+ window).takeRight(maxWindows)
    window
  }

  /** The workload over the retained windows. */
  def snapshot(): WorkloadSnapshot = synchronized {
    val totals = windows.iterator.flatMap(_.shapes.valuesIterator).foldLeft(Map.empty[Long, ShapeDelta]) { (acc, d) =>
      acc.updated(d.shapeId, acc.get(d.shapeId).fold(d)(_.plus(d)))
    }
    WorkloadSnapshot(windows, totals)
  }

  /** Number of windows currently retained. */
  def windowCount: Int = synchronized(windows.size)

  /** Forgets every window and the baseline. */
  def clear(): Unit = synchronized {
    previous = Map.empty
    lastAt = None
    windows = Vector.empty
  }

  /** The change of one shape between two readings, or `None` if it did not run. */
  private def delta(before: Option[ShapeStats], now: ShapeStats): Option[ShapeDelta] =
    // A reset (any counter smaller than before) makes the new counters the whole interval.
    val base = before.filter(b => now.executions >= b.executions && now.totalNanos >= b.totalNanos && now.totalRowsRead >= b.totalRowsRead)
    val executions = now.executions - base.fold(0L)(_.executions)
    if executions <= 0L then None
    else
      Some(ShapeDelta(
        now.shapeId,
        executions,
        now.failures - base.fold(0L)(_.failures),
        now.totalNanos - base.fold(0L)(_.totalNanos),
        now.totalActualRows - base.fold(0L)(_.totalActualRows),
        now.totalRowsRead - base.fold(0L)(_.totalRowsRead),
        now.maxQError
      ))
