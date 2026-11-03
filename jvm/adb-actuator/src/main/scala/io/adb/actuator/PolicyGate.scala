package io.adb.actuator

import io.adb.advisor.{CreateIndex, Informational, PhysicalDesign, Recommendation, RunAnalyze, WorkloadSnapshot}

/** Why the gate refused a recommendation. */
enum RejectReason derives CanEqual:
  /** The recommendation is information, or has no statement the actuator can run. */
  case NotActionable
  /** The index the recommendation asks for already exists. */
  case AlreadyExists
  /** The shapes behind it ran fewer than `minExecutions` times. */
  case TooFewExecutions
  /** Its benefit/cost ratio is below `minRatio`. */
  case BelowRatio
  /** A person dropped an index the advisor had created, so it is not recreated. */
  case ExternallyDropped
  /** Another action on the same target came less than `cooldown` ago. */
  case Cooldown
  /** The advisor rolled this target back less than `rollbackCooldown` ago. */
  case RollbackCooldown
  /** The advisor already owns `maxAdvisorIndexes` indexes. */
  case TooManyIndexes
  /** The index would push the advisor's indexes over `maxIndexBytes`. */
  case TooManyBytes
  /** The table is larger than `maxBuildRows`; a person has to decide. */
  case TableTooLarge
  /** `maxConcurrentBuilds` builds are already running. */
  case BuildInFlight

  /** Lower-case label for reports (`too_few_executions`). */
  def label: String = toString.flatMap(c => if c.isUpper then s"_${c.toLower}" else c.toString).drop(1)

/** The gate's answer. */
sealed trait GateDecision derives CanEqual

/** The recommendation may be carried out. */
case object Approve extends GateDecision

/** The recommendation is refused; nothing was changed.
  *
  * @param reason the rule that refused it
  * @param detail the figures, in words, for the action log
  */
final case class Reject(reason: RejectReason, detail: String) extends GateDecision

/** What the gate needs to know about the actuator's own state when it judges one recommendation.
  * Plain values, built by the controller from the ownership registry and the action history, so the
  * gate itself touches nothing.
  *
  * @param nowNanos            the controller's clock
  * @param ownedIndexes        indexes the advisor owns now
  * @param ownedBytes          their estimated size
  * @param buildsInFlight      index builds running now
  * @param lastActionNanos     time of the latest action on this target, if any
  * @param lastRollbackNanos   time of the latest rollback of this target, if any
  * @param externallyDropped   whether a person dropped an advisor index on this target
  */
final case class GateContext(
    nowNanos: Long,
    ownedIndexes: Int = 0,
    ownedBytes: Long = 0L,
    buildsInFlight: Int = 0,
    lastActionNanos: Option[Long] = None,
    lastRollbackNanos: Option[Long] = None,
    externallyDropped: Boolean = false
) derives CanEqual

/** The decision whether a recommendation may become an action.
  *
  * **Pure.** [[decide]] reads only its arguments and returns data; a rejection has therefore no
  * side effect by construction, which is the first safety rule of `AUTO_SAFE` (budget first). The
  * checks run from the cheapest and most basic to the budget, and the first that fails names the
  * reason.
  */
object PolicyGate:
  /** Decides on `recommendation`.
    *
    * @param recommendation what the advisor shows
    * @param design         the physical design now
    * @param workload       the workload the evidence is read from
    * @param policy         thresholds
    * @param budget         limits
    * @param context        the actuator's own state
    */
  def decide(
      recommendation: Recommendation,
      design: PhysicalDesign,
      workload: WorkloadSnapshot,
      policy: Policy,
      budget: Budget,
      context: GateContext
  ): GateDecision = recommendation match
    case _: Informational => Reject(RejectReason.NotActionable, "information only, no statement to run")
    case index: CreateIndex => decideIndex(index, design, workload, policy, budget, context)
    case analyze: RunAnalyze => decideAnalyze(analyze, workload, policy, context)

  /** Captured executions of the shapes behind a recommendation, over the retained windows. */
  def evidenceExecutions(shapeIds: Vector[Long], workload: WorkloadSnapshot): Long =
    shapeIds.iterator.map(id => workload.shapes.get(id).fold(0L)(_.executions)).sum

  /** The most rows one execution of the evidence shapes scanned, a stand-in for the table size
    * when the table was never analyzed.
    */
  private def scannedRows(shapeIds: Vector[Long], workload: WorkloadSnapshot): Long =
    shapeIds.iterator.map(id => workload.shapes.get(id).fold(0L)(d => math.round(d.meanRowsRead))).maxOption.getOrElse(0L)

  /** Cooldown checks shared by every kind of action. */
  private def cooling(policy: Policy, context: GateContext): Option[Reject] =
    context.lastRollbackNanos.filter(t => context.nowNanos - t < policy.rollbackCooldownNanos).map { t =>
      Reject(RejectReason.RollbackCooldown, s"rolled back ${seconds(context.nowNanos - t)} s ago, waits ${seconds(policy.rollbackCooldownNanos)} s")
    }.orElse(context.lastActionNanos.filter(t => context.nowNanos - t < policy.cooldownNanos).map { t =>
      Reject(RejectReason.Cooldown, s"last action ${seconds(context.nowNanos - t)} s ago, waits ${seconds(policy.cooldownNanos)} s")
    })

  private def seconds(nanos: Long): Long = nanos / 1_000_000_000L

  /** The rules for `CREATE INDEX`. */
  private def decideIndex(
      r: CreateIndex,
      design: PhysicalDesign,
      workload: WorkloadSnapshot,
      policy: Policy,
      budget: Budget,
      context: GateContext
  ): GateDecision =
    val executions = evidenceExecutions(r.shapeIds, workload)
    val entity = design.entity(r.entityId)
    val rows = math.max(entity.fold(0L)(_.rowCount), scannedRows(r.shapeIds, workload))
    val ratio = r.score.fold(0.0)(_.ratio)
    if r.fields.headOption.exists(f => entity.exists(_.hasLeadingIndexOn(f))) then
      Reject(RejectReason.AlreadyExists, "an index with this leading column exists")
    else if executions < policy.minExecutions then
      Reject(RejectReason.TooFewExecutions, s"$executions executions, needs ${policy.minExecutions}")
    else if ratio < policy.minRatio then
      Reject(RejectReason.BelowRatio, f"benefit/cost $ratio%.1f, needs ${policy.minRatio}%.1f")
    else if context.externallyDropped then
      Reject(RejectReason.ExternallyDropped, "a person dropped the index the advisor created here")
    else cooling(policy, context).getOrElse {
      if context.ownedIndexes + 1 > budget.maxAdvisorIndexes then
        Reject(RejectReason.TooManyIndexes, s"${context.ownedIndexes} advisor indexes, limit ${budget.maxAdvisorIndexes}")
      else if rows > budget.maxBuildRows then
        Reject(RejectReason.TableTooLarge, s"$rows rows, the advisor builds up to ${budget.maxBuildRows}")
      else if context.ownedBytes + budget.estimateBytes(rows) > budget.maxIndexBytes then
        Reject(RejectReason.TooManyBytes, s"${context.ownedBytes + budget.estimateBytes(rows)} bytes would exceed ${budget.maxIndexBytes}")
      else if context.buildsInFlight >= budget.maxConcurrentBuilds then
        Reject(RejectReason.BuildInFlight, s"${context.buildsInFlight} builds running, limit ${budget.maxConcurrentBuilds}")
      else Approve
    }

  /** The rules for `ANALYZE`: evidence and cooldown only (it builds nothing and is not undone). */
  private def decideAnalyze(r: RunAnalyze, workload: WorkloadSnapshot, policy: Policy, context: GateContext): GateDecision =
    val executions = evidenceExecutions(r.shapeIds, workload)
    if executions < policy.minExecutions then
      Reject(RejectReason.TooFewExecutions, s"$executions executions, needs ${policy.minExecutions}")
    else cooling(policy, context).getOrElse(Approve)
