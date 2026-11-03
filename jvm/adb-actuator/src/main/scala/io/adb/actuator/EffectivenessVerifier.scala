package io.adb.actuator

import io.adb.advisor.{ShapeDelta, WorkloadSnapshot}
import io.adb.telemetry.WriteStats

/** What the workload did after an action: the windows that started at or after it.
  *
  * @param windows number of whole windows in the period
  * @param shapes  what each shape did in them
  * @param writes  what the writes to each entity cost in them
  */
final case class AfterPeriod(windows: Int, shapes: Map[Long, ShapeDelta], writes: Map[Long, WriteStats]) derives CanEqual

/** Constructors of [[AfterPeriod]]. */
object AfterPeriod:
  /** The workload since `sinceNanos`. A window counts only if it started at or after that time, so a
    * window that was already running when the action was applied (and mixes before and after) is left out.
    */
  def since(workload: WorkloadSnapshot, sinceNanos: Long): AfterPeriod =
    val windows = workload.windows.filter(_.startedAtNanos >= sinceNanos)
    val shapes = windows.iterator.flatMap(_.shapes.valuesIterator).foldLeft(Map.empty[Long, ShapeDelta]) { (acc, d) =>
      acc.updated(d.shapeId, acc.get(d.shapeId).fold(d)(_.plus(d)))
    }
    val writes = windows.iterator.flatMap(_.writes.valuesIterator).foldLeft(Map.empty[Long, WriteStats]) { (acc, w) =>
      acc.updated(w.entityId, acc.get(w.entityId).fold(w)(a =>
        WriteStats(w.entityId, a.statements + w.statements, a.rows + w.rows, a.totalNanos + w.totalNanos)))
    }
    AfterPeriod(windows.size, shapes, writes)

/** The verifier's judgement of an action.
  *
  * - [[NotYet]]: too early to say.
  * - [[Inconclusive]]: enough time passed but no shape ran often enough both before and after.
  * - [[Keep]]: the workload got measurably cheaper.
  * - [[Undo]]: it did not, so the action should be taken back.
  */
sealed trait Verdict derives CanEqual

/** Fewer than `verifyWindows` windows have passed.
  *
  * @param detail how far along it is
  */
final case class NotYet(detail: String) extends Verdict

/** No shape has `minExecutions` runs in both periods, so nothing can be concluded.
  *
  * @param detail what is missing
  */
final case class Inconclusive(detail: String) extends Verdict

/** Rows read per execution fell enough and time did not get clearly worse.
  *
  * @param rowsReduction factor by which rows read per execution fell (3.0 = to a third)
  * @param timeRatio     time per execution after / before (below 1 is faster)
  * @param shapes        shapes the verdict rests on
  */
final case class Keep(rowsReduction: Double, timeRatio: Double, shapes: Int) extends Verdict

/** The action did not pay.
  *
  * @param why           the rule that failed
  * @param rowsReduction factor by which rows read per execution fell
  * @param timeRatio     time per execution after / before
  */
final case class Undo(why: UndoReason, rowsReduction: Double, timeRatio: Double) extends Verdict

/** Why an action is taken back. */
enum UndoReason derives CanEqual:
  /** Rows read per execution did not fall by `minRowsReduction`. */
  case NoRowsReduction
  /** Rows fell, but the same shapes got clearly slower. */
  case SlowerDespiteFewerRows
  /** The index cost its table's writes more than the reads gained. */
  case WriteRegression
  /** No captured shape used it for `idleWindows` windows. */
  case Idle

  /** Lower-case label for reports. */
  def label: String = this match
    case NoRowsReduction => "no_rows_reduction"
    case SlowerDespiteFewerRows => "slower_despite_fewer_rows"
    case WriteRegression => "write_regression"
    case Idle => "idle"

/** Judges whether an action made the workload it was meant to help cheaper.
  *
  * **Rows read per execution is the primary evidence.** The engine counts it exactly, and it does not
  * depend on how busy the machine is. **Elapsed time is secondary**: it is noisy, so it can veto a
  * result only when it is clearly worse (`timeTolerance`, 2x by default), never overturn a good one
  * on a small difference. Both are per execution, over the same shapes before and after, weighted by
  * how often each ran afterwards, so a change in query rate is neither a win nor a loss.
  */
object EffectivenessVerifier:
  /** Judges `baseline` against `after`.
    *
    * @param baseline figures before the action
    * @param after    the workload since the action
    * @param policy   thresholds
    */
  def verify(baseline: Baseline, after: AfterPeriod, policy: Policy): Verdict =
    if after.windows < policy.verifyWindows then NotYet(s"${after.windows} of ${policy.verifyWindows} windows")
    else
      val pairs = baseline.usableShapes(policy.minExecutions).toVector.flatMap { (id, before) =>
        after.shapes.get(id).filter(_.executions >= policy.minExecutions).map(d => (before, ShapeFigures.of(d)))
      }
      if pairs.isEmpty then Inconclusive(s"no shape ran at least ${policy.minExecutions} times both before and after")
      else
        // Weights are the executions afterwards: the figures describe the workload as it runs now.
        val weight = pairs.map(_._2.executions.toDouble)
        val rowsBefore = pairs.zip(weight).map { case ((b, _), w) => b.meanRowsRead * w }.sum
        val rowsAfter = pairs.zip(weight).map { case ((_, a), w) => a.meanRowsRead * w }.sum
        val timeBefore = pairs.zip(weight).map { case ((b, _), w) => b.meanNanos * w }.sum
        val timeAfter = pairs.zip(weight).map { case ((_, a), w) => a.meanNanos * w }.sum
        val reduction = if rowsAfter <= 0.0 then (if rowsBefore > 0.0 then Double.PositiveInfinity else 1.0) else rowsBefore / rowsAfter
        val timeRatio = if timeBefore <= 0.0 then 1.0 else timeAfter / timeBefore
        if reduction < policy.minRowsReduction then Undo(UndoReason.NoRowsReduction, reduction, timeRatio)
        else if timeRatio > policy.timeTolerance then Undo(UndoReason.SlowerDespiteFewerRows, reduction, timeRatio)
        else Keep(reduction, timeRatio, pairs.size)
