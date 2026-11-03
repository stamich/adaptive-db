package io.adb.actuator

/** The outcome of comparing an entity's write cost before and after an index was built. */
sealed trait WriteCost derives CanEqual

/** Too few writes before or after to say anything (the counters are per entity and per period). */
case object WriteCostUnmeasured extends WriteCost

/** Writes did not slow down by more than `maxWriteSlowdown`, or the index pays for the slowdown.
  *
  * @param slowdown rise of the mean write time (0.1 = 10 % slower; negative = faster)
  */
final case class WriteCostAcceptable(slowdown: Double) extends WriteCost

/** Writes slowed down by more than `maxWriteSlowdown` and the reads did not gain enough to make up for it.
  *
  * @param slowdown        rise of the mean write time
  * @param readSavedNanos  time per period the reads of the evidence shapes saved
  * @param writeAddedNanos time per period the writes lost
  */
final case class WriteCostRegression(slowdown: Double, readSavedNanos: Double, writeAddedNanos: Double) extends WriteCost

/** Measures what an index costs the writes of its table (2.5.4).
  *
  * 2.5.3 had to assume a flat write penalty because telemetry saw no writes. With per-entity write
  * counters the actuator compares the mean time of a write statement before and after the index.
  * A slowdown alone is not enough to roll back: a 40 % slower write is a good bargain if the reads
  * got a hundred times cheaper. The index is rolled back only when the writes slowed down by more
  * than `maxWriteSlowdown` **and** the time the writes lost exceeds the time the reads saved.
  */
object WriteCostCheck:
  /** Judges the write cost of the index on `entityId`.
    *
    * @param baseline figures before (its [[Baseline.writes]] and shapes)
    * @param after    the workload since the index was built
    * @param entityId entity the index is on
    * @param policy   thresholds
    */
  def check(baseline: Baseline, after: AfterPeriod, entityId: Long, policy: Policy): WriteCost =
    val before = baseline.writes.filter(_.statements >= policy.minExecutions)
    val now = after.writes.get(entityId).filter(_.statements >= policy.minExecutions)
    (before, now) match
      case (Some(b), Some(a)) if b.meanNanos > 0.0 =>
        val slowdown = a.meanNanos / b.meanNanos - 1.0
        if slowdown <= policy.maxWriteSlowdown then WriteCostAcceptable(slowdown)
        else
          val saved = baseline.usableShapes(policy.minExecutions).iterator.flatMap { (id, beforeShape) =>
            after.shapes.get(id).map(d => (beforeShape.meanNanos - d.meanNanos) * d.executions)
          }.sum
          val added = (a.meanNanos - b.meanNanos) * a.statements
          if saved >= added then WriteCostAcceptable(slowdown) else WriteCostRegression(slowdown, saved, added)
      case _ => WriteCostUnmeasured
