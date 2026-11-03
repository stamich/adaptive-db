package io.adb.actuator

import io.adb.advisor.{ShapeDelta, WorkloadSnapshot}

/** Per-execution figures of one query shape over a period.
  *
  * Per execution, not totals: a change in how often the query runs must not look like a win or a loss.
  *
  * @param shapeId      the shape
  * @param executions   captured executions in the period
  * @param meanRowsRead rows scanned per execution (the engine's exact counter)
  * @param meanNanos    elapsed nanoseconds per execution
  */
final case class ShapeFigures(shapeId: Long, executions: Long, meanRowsRead: Double, meanNanos: Double) derives CanEqual

/** Constructors of [[ShapeFigures]]. */
object ShapeFigures:
  /** The figures of a workload delta. */
  def of(delta: ShapeDelta): ShapeFigures = ShapeFigures(delta.shapeId, delta.executions, delta.meanRowsRead, delta.meanNanos)

/** Write figures of an entity over a period.
  *
  * @param statements write statements in the period
  * @param meanNanos  elapsed nanoseconds per statement
  */
final case class WriteFigures(statements: Long, meanNanos: Double) derives CanEqual

/** What the workload looked like just before an action: the yardstick its effect is measured against.
  *
  * @param takenAtNanos clock reading when it was taken
  * @param shapes       figures of the shapes that justified the action
  * @param writes       write figures of the entity, if telemetry saw writes to it
  */
final case class Baseline(takenAtNanos: Long, shapes: Map[Long, ShapeFigures], writes: Option[WriteFigures]) derives CanEqual:
  /** Shapes with at least `minExecutions` runs: the only ones a verdict may rest on. */
  def usableShapes(minExecutions: Long): Map[Long, ShapeFigures] = shapes.filter(_._2.executions >= minExecutions)

/** Takes baselines from the workload windows. */
object BaselineCapture:
  /** The baseline for an action on `entityId` justified by `shapeIds`.
    *
    * @param shapeIds shapes whose evidence led to the action
    * @param entityId entity the action changes (for its write figures)
    * @param workload the workload before the action
    * @param nowNanos clock reading
    */
  def take(shapeIds: Vector[Long], entityId: Long, workload: WorkloadSnapshot, nowNanos: Long): Baseline =
    val shapes = shapeIds.flatMap(id => workload.shapes.get(id)).map(d => d.shapeId -> ShapeFigures.of(d)).toMap
    val writes = workload.writes.get(entityId).filter(_.statements > 0L).map(w => WriteFigures(w.statements, w.meanNanos))
    Baseline(nowNanos, shapes, writes)
