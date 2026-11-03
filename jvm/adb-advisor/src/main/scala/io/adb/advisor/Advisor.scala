package io.adb.advisor

import io.adb.telemetry.{QueryShape, ShapeStats, TelemetryRuntime, WriteStats}

/** All knobs of the advisor.
  *
  * @param windows        time windows the workload model keeps (default 60)
  * @param confirmAfter   evaluations in a row before a finding is shown (default 3)
  * @param withdrawAfter  evaluations in a row before a shown finding is withdrawn (default 3)
  * @param auditCapacity  audit log entries kept (default 200)
  * @param model          cost-benefit parameters
  * @param detectors      detector thresholds
  */
final case class AdvisorConfig(
    windows: Int = 60,
    confirmAfter: Int = 3,
    withdrawAfter: Int = 3,
    auditCapacity: Int = 200,
    model: CostBenefitModel = CostBenefitModel(),
    detectors: DetectorConfig = DetectorConfig()
) derives CanEqual

/** The result of one evaluation: immutable, safe to hand to any thread or to print.
  *
  * @param evaluation      1-based number of this evaluation
  * @param atNanos         clock reading of the evaluation
  * @param windows         workload windows the evaluation looked at
  * @param spanNanos       time those windows cover
  * @param executions      captured executions in them
  * @param recommendations the advice shown now, best cost-benefit ratio first
  * @param changes         what changed in this evaluation (confirmed, withdrawn, applied)
  */
final case class AdvisorReport(
    evaluation: Long,
    atNanos: Long,
    windows: Int,
    spanNanos: Long,
    executions: Long,
    recommendations: Vector[Recommendation],
    changes: Vector[AuditEntry]
) derives CanEqual

/** The workload advisor: reads telemetry, finds problems the physical design could fix, and says so.
  *
  * **It only recommends.** It never runs a statement, creates an index or changes a setting. It
  * reads three things, all immutable values: the per-shape counters (`stats`), the shape
  * definitions (`shapes`) and the [[PhysicalDesign]] its caller passes to [[evaluate]]. It never
  * reads the telemetry queue or any live engine structure, so an evaluation cannot disturb queries.
  *
  * Each [[evaluate]] closes one workload window, runs the [[Detectors]], passes the findings
  * through [[Hysteresis]] and records state changes in the [[AuditLog]]. Call it on a timer
  * ([[AdvisorScheduler]]) or by hand.
  *
  * @param stats     returns the cumulative per-shape counters (see [[Advisor.forTelemetry]])
  * @param shapes    looks a shape up by id
  * @param writes    returns the cumulative per-entity write counters (2.5.4); empty by default
  * @param config    knobs; also readable afterwards as `advisor.config`
  * @param clock     monotonic clock in nanoseconds (replaceable for tests)
  * @param detectors the detectors to run, in report order (default: all four)
  */
final class Advisor(
    stats: () => Vector[ShapeStats],
    shapes: Long => Option[QueryShape],
    val config: AdvisorConfig = AdvisorConfig(),
    clock: () => Long = () => System.nanoTime(),
    detectors: Vector[Detector] = Detectors.all,
    writes: () => Vector[WriteStats] = () => Vector.empty
):
  private val workload = new WorkloadModel(config.windows)
  private val hysteresis = new Hysteresis(config.confirmAfter, config.withdrawAfter)

  /** Every state change of every recommendation since the advisor was created. */
  val auditLog: AuditLog = new AuditLog(config.auditCapacity)

  /** Number of evaluations made so far. */
  private var evaluations = 0L
  /** The latest report, or `None` before the first evaluation. */
  @volatile private var latest: Option[AdvisorReport] = None

  /** Evaluates the workload against `design`.
    *
    * @param design the physical design now (indexes and statistics)
    * @return the report; also kept as [[lastReport]]
    */
  def evaluate(design: PhysicalDesign): AdvisorReport = synchronized {
    val now = clock()
    workload.observe(stats(), now, writes())
    val snapshot = workload.snapshot()
    val context = DetectorContext(snapshot, shapes, design, config.model, config.detectors)
    val found = detectors.flatMap(_.detect(context))
    val changes = hysteresis.update(found, applied(design)).map((kind, r) => auditLog.record(kind, r, now))
    evaluations += 1
    val report = AdvisorReport(
      evaluations,
      now,
      snapshot.windows.size,
      snapshot.spanNanos,
      snapshot.totalExecutions,
      hysteresis.shown.sortBy(r => (-r.score.fold(0.0)(_.ratio), r.key)),
      changes
    )
    latest = Some(report)
    report
  }

  /** The workload over the windows kept now: per-shape deltas and per-entity write figures.
    * An immutable copy; the actuator reads it to take baselines (2.5.4).
    */
  def workloadSnapshot(): WorkloadSnapshot = workload.snapshot()

  /** The report of the latest [[evaluate]], if there was one. */
  def lastReport: Option[AdvisorReport] = latest

  /** Forgets the workload and every finding; the audit log is kept. */
  def reset(): Unit = synchronized {
    workload.clear()
    hysteresis.clear()
    latest = None
  }

  /** Whether `recommendation` has been carried out in `design`: an index on its first column exists. */
  private def applied(design: PhysicalDesign)(recommendation: Recommendation): Boolean = recommendation match
    case c: CreateIndex => c.fields.headOption.exists(f => design.entity(c.entityId).exists(_.hasLeadingIndexOn(f)))
    case _ => false

/** Constructors of [[Advisor]]. */
object Advisor:
  /** An advisor reading `runtime`'s folded per-shape counters and shape registry (not its queue). */
  def forTelemetry(runtime: TelemetryRuntime, config: AdvisorConfig = AdvisorConfig()): Advisor =
    new Advisor(() => runtime.allStats(), runtime.registry.get, config, writes = () => runtime.writes.all())
