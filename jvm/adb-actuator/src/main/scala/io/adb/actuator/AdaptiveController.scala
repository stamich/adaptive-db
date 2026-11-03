package io.adb.actuator

import io.adb.advisor.{Advisor, AdvisorReport, CreateIndex, PhysicalDesign, Recommendation, RunAnalyze}
import io.adb.telemetry.QueryShape

/** What one [[AdaptiveController.tick]] did.
  *
  * @param report the advisor's report for this tick, if it evaluated (not in [[AdaptiveMode.Off]])
  * @param events what the action log recorded during the tick, oldest first
  */
final case class TickResult(report: Option[AdvisorReport], events: Vector[ActionLogEntry]) derives CanEqual

/** The loop that closes 2.5.4: **observe → recommend → decide → act → measure → keep or undo**.
  *
  * One [[tick]] is one pass, run on the advisor's timer. In [[AdaptiveMode.AutoSafe]] it
  *
  *  1. evaluates the advisor (which closes one workload window);
  *  2. adopts `adv_*` indexes it finds that it does not know, and notices owned indexes a person dropped;
  *  3. verifies the actions still being measured, keeping those that paid and undoing those that did not;
  *  4. drops advisor indexes that nobody has wanted for `idleWindows` windows;
  *  5. takes the best recommendation the [[PolicyGate]] approves and applies it, **at most one per tick**.
  *
  * In [[AdaptiveMode.Advisory]] it only evaluates, exactly as 2.5.3; in [[AdaptiveMode.Off]] it does
  * nothing. Changing the mode takes effect at the next tick, and a tick is atomic with respect to
  * mode changes only in the sense that an action already running finishes or fails by itself (the
  * engine commits each statement atomically).
  *
  * The controller never touches the database except through the [[Actuator]], which it wraps in an
  * [[OwnedOnlyActuator]] so that no drop can reach an index it does not own.
  *
  * @param advisor  the advisor to evaluate and read the workload from
  * @param actuator carries actions out (the gateway's, or a fake in tests)
  * @param shapes   looks a query shape up by id (for the idle rule)
  * @param config   policy, budget and log sizes
  * @param clock    monotonic clock in nanoseconds (replaceable for tests)
  */
final class AdaptiveController(
    val advisor: Advisor,
    actuator: Actuator,
    shapes: Long => Option[QueryShape],
    val config: AdaptiveConfig = AdaptiveConfig(),
    clock: () => Long = () => System.nanoTime()
) extends AutoCloseable:
  private def policy = config.policy
  private def budget = config.budget

  /** What the advisor owns. */
  val registry: OwnershipRegistry = new OwnershipRegistry
  /** What the actuator did and refused. */
  val log: ActionLog = new ActionLog(config.logCapacity)
  /** Last action and rollbacks per target. */
  val history: ActionHistory = new ActionHistory
  /** Bounds the builds. */
  val buildGuard: BuildGuard = new BuildGuard(budget.maxConcurrentBuilds)

  private val safeActuator: Actuator = new OwnedOnlyActuator(actuator, registry)
  private val idle = new IdleDetector(policy.idleWindows)

  @volatile private var currentMode: AdaptiveMode = AdaptiveMode.Off
  /** Actions, oldest first. */
  private var actionList: Vector[Action] = Vector.empty
  private var nextId = 1L
  /** Last rejection reason logged per recommendation key (so a repeated rejection is logged once). */
  private var lastRejection: Map[String, RejectReason] = Map.empty
  /** Indexes whose rollback failed and was logged (so a persistent failure is logged once). */
  private var rollbackFailed: Set[String] = Set.empty
  /** Events recorded during the current tick. */
  private val tickEvents = Vector.newBuilder[ActionLogEntry]

  /** The current mode. */
  def mode: AdaptiveMode = currentMode

  /** Changes the mode; takes effect at the next tick. Leaving [[AdaptiveMode.AutoSafe]] stops all new actions and rollbacks at once. */
  def mode_=(next: AdaptiveMode): Unit = currentMode = next

  /** The actions, oldest first. */
  def actions: Vector[Action] = synchronized(actionList)

  /** Actions applied and still being measured. */
  def pending: Vector[Action] = synchronized(actionList.filter(_.state == ActionState.Applied))

  /** Refuses new index builds from now on (database shutdown). A build already running finishes. */
  def close(): Unit = buildGuard.shutdown()

  /** One pass of the loop; see the class documentation.
    *
    * @param design the physical design now
    */
  def tick(design: PhysicalDesign): TickResult = synchronized {
    tickEvents.clear()
    val report = currentMode match
      case AdaptiveMode.Off => None
      case AdaptiveMode.Advisory => Some(advisor.evaluate(design))
      case AdaptiveMode.AutoSafe =>
        val report = advisor.evaluate(design)
        autoSafe(design, report)
        Some(report)
    TickResult(report, tickEvents.result())
  }

  /** The `AutoSafe` part of a tick. */
  private def autoSafe(design: PhysicalDesign, report: AdvisorReport): Unit =
    val now = clock()
    val workload = advisor.workloadSnapshot()
    if policy.adoptByPrefix then
      registry.adopt(design, now, budget).foreach(o => note(ActionEvent.Adopted, now, None, s"adopt ${o.name}", "found by its adv_ name; not verified, idle rule applies"))
    registry.reconcile(design).foreach(o => droppedByPerson(o, now))
    verifyPending(workload, now)
    dropIdle(workload, now)
    approveOne(design, report, workload, now)

  // ---- recording -------------------------------------------------------------------------

  private def note(event: ActionEvent, now: Long, id: Option[ActionId], what: String, detail: String): Unit =
    tickEvents += log.record(event, now, id, what, detail)

  private def newId(): ActionId =
    val id = ActionId(nextId)
    nextId += 1
    id

  /** Keeps the action list bounded: the oldest final actions go first. */
  private def remember(action: Action): Unit =
    actionList = actionList :+ action
    if actionList.size > config.maxActions then
      val dropAt = actionList.indexWhere(_.state.isFinal)
      if dropAt >= 0 then actionList = actionList.patch(dropAt, Nil, 1)

  private def update(id: ActionId, f: Action => Action): Unit =
    actionList = actionList.map(a => if a.id == id then f(a) else a)

  // ---- steps -----------------------------------------------------------------------------

  /** A person dropped an index the advisor owned. */
  private def droppedByPerson(owned: OwnedIndex, now: Long): Unit =
    idle.forget(owned.name)
    owned.createdBy.foreach { id =>
      update(id, a => if a.state == ActionState.Applied || a.state == ActionState.Kept then a.moved(ActionState.RolledBack, "dropped by a person", now) else a)
    }
    note(ActionEvent.ExternallyDropped, now, owned.createdBy, s"index ${owned.name}", "dropped by a person; the advisor will not recreate it")

  /** Judges the actions that are still being measured. */
  private def verifyPending(workload: io.adb.advisor.WorkloadSnapshot, now: Long): Unit =
    actionList.filter(_.state == ActionState.Applied).foreach { action =>
      for
        name <- action.indexName
        owned <- registry.get(name)
        baseline <- owned.baseline
      do
        val after = AfterPeriod.since(workload, owned.appliedAtNanos)
        EffectivenessVerifier.verify(baseline, after, policy) match
          case NotYet(_) => ()
          case Inconclusive(why) =>
            if after.windows >= 2 * policy.verifyWindows then
              update(action.id, _.moved(ActionState.Kept, s"unverified: $why", now))
              note(ActionEvent.Unverified, now, Some(action.id), action.summary, s"$why; kept under the idle rule")
          case Keep(reduction, timeRatio, shapeCount) =>
            WriteCostCheck.check(baseline, after, owned.entityId, policy) match
              case WriteCostRegression(slowdown, saved, added) =>
                rollback(owned, UndoReason.WriteRegression, f"writes ${slowdown * 100}%.0f%% slower; lost ${added / 1e6}%.1f ms, reads saved ${saved / 1e6}%.1f ms", now)
              case _ =>
                update(action.id, _.moved(ActionState.Kept, f"rows read fell ${reduction}%.1fx, time x$timeRatio%.2f", now))
                note(ActionEvent.Kept, now, Some(action.id), action.summary, f"rows read per execution fell ${reduction}%.1fx, time x$timeRatio%.2f, over $shapeCount shape(s)")
          case Undo(why, reduction, timeRatio) =>
            rollback(owned, why, f"rows read fell only ${reduction}%.2fx, time x$timeRatio%.2f", now)
    }

  /** Drops advisor indexes nobody has wanted for `idleWindows` windows. */
  private def dropIdle(workload: io.adb.advisor.WorkloadSnapshot, now: Long): Unit =
    workload.windows.lastOption.foreach { window =>
      val pendingNames = actionList.filter(_.state == ActionState.Applied).flatMap(_.indexName).map(_.toLowerCase).toSet
      val candidates = registry.owned.filterNot(o => pendingNames.contains(o.name.toLowerCase))
      idle.observe(candidates, window, shapes).foreach { owned =>
        rollback(owned, UndoReason.Idle, s"no query wanted it for ${policy.idleWindows} windows", now)
      }
    }

  /** Drops an owned index and records why. */
  private def rollback(owned: OwnedIndex, why: UndoReason, detail: String, now: Long): Unit =
    if currentMode == AdaptiveMode.AutoSafe then
      RollbackPlanner.plan(registry, owned.name, s"${why.label}: $detail", newId(), now) match
        case Left(message) => note(ActionEvent.Failed, now, owned.createdBy, s"drop_index ${owned.name}", message)
        case Right(drop) =>
          safeActuator(drop) match
            case Done =>
              registry.release(owned.name)
              idle.forget(owned.name)
              rollbackFailed -= owned.name.toLowerCase
              history.recordRollback(owned.target, now)
              owned.createdBy.foreach(id => update(id, _.moved(ActionState.RolledBack, s"${why.label}: $detail", now)))
              remember(drop.moved(ActionState.Kept, why.label, now))
              note(ActionEvent.RolledBack, now, owned.createdBy, s"drop_index ${owned.name}", s"${why.label}: $detail")
            case ActuationFailed(message) =>
              if !rollbackFailed.contains(owned.name.toLowerCase) then
                rollbackFailed += owned.name.toLowerCase
                note(ActionEvent.Failed, now, owned.createdBy, s"drop_index ${owned.name}", s"rollback failed, will retry: $message")

  /** Gates the recommendations best first and applies the first approved one. */
  private def approveOne(design: PhysicalDesign, report: AdvisorReport, workload: io.adb.advisor.WorkloadSnapshot, now: Long): Unit =
    val actionable = report.recommendations.filter {
      case _: CreateIndex | _: RunAnalyze => true
      case _ => false
    }
    // Forget the rejections of recommendations that are gone, so a return is logged again.
    lastRejection = lastRejection.filter((key, _) => actionable.exists(_.key == key))
    val pendingBuilds = buildGuard.inFlight
    var done = false
    actionable.foreach { rec =>
      if !done then
        val key = rec.key
        val context = GateContext(
          now, registry.count, registry.bytes, pendingBuilds,
          history.lastAction(key), history.lastRollback(key),
          registry.externallyDropped(key), history.rollbacks(key))
        PolicyGate.decide(rec, design, workload, policy, budget, context) match
          case Reject(reason, detail) =>
            if !lastRejection.get(key).contains(reason) then
              lastRejection = lastRejection.updated(key, reason)
              note(ActionEvent.Rejected, now, None, rec.summary, s"${reason.label}: $detail")
          case Approve =>
            lastRejection -= key
            done = carryOut(rec, design, workload, now)
    }

  /** Applies an approved recommendation. @return whether an action was started (so the tick stops) */
  private def carryOut(rec: Recommendation, design: PhysicalDesign, workload: io.adb.advisor.WorkloadSnapshot, now: Long): Boolean =
    val id = newId()
    val action = rec match
      case c: CreateIndex =>
        Action(id, ActionKind.CreateIndex, ActionTarget(c.entityId, c.key), c.entityName, c.sql.get, Some(s"DROP INDEX ${c.indexName}"),
          Some(c.indexName), ActionState.Proposed, "approved by the policy gate", c.shapeIds, now, now)
      case a: RunAnalyze =>
        Action(id, ActionKind.Analyze, ActionTarget(a.entityId, a.key), a.entityName, a.sql.get, None,
          None, ActionState.Proposed, "approved by the policy gate", a.shapeIds, now, now)
      case other => throw new IllegalStateException(s"not actionable: $other")
    val baseline = BaselineCapture.take(action.shapeIds, action.target.entityId, workload, now)
    val outcome = buildGuard.guarded(safeActuator(action))
    outcome match
      case None =>
        // The guard refused (shutdown or a build in flight): nothing was tried.
        note(ActionEvent.Rejected, now, None, rec.summary, s"${RejectReason.BuildInFlight.label}: build guard closed or busy")
        false
      case Some(Done) =>
        val appliedAt = clock()
        history.recordAction(action.target, appliedAt)
        action.kind match
          case ActionKind.CreateIndex =>
            val rows = design.entity(action.target.entityId).fold(0L)(_.rowCount)
            registry.record(OwnedIndex(
              action.indexName.get, action.target.entityId, action.entityName,
              rec.asInstanceOf[CreateIndex].fields, action.target, Some(id), appliedAt,
              budget.estimateBytes(math.max(rows, 0L)), baseline = Some(baseline), appliedAtNanos = appliedAt))
            remember(action.moved(ActionState.Applied, "built; being measured", appliedAt))
            note(ActionEvent.Applied, appliedAt, Some(id), action.summary, action.sql)
          case _ =>
            remember(action.moved(ActionState.Kept, "analyze needs no verification", appliedAt))
            note(ActionEvent.Applied, appliedAt, Some(id), action.summary, action.sql)
            note(ActionEvent.Kept, appliedAt, Some(id), action.summary, "analyze needs no verification")
        true
      case Some(ActuationFailed(message)) =>
        history.recordAction(action.target, now)
        remember(action.moved(ActionState.Failed, message, now))
        note(ActionEvent.Failed, now, Some(id), action.summary, message)
        true
