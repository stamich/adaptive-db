package io.adb.gateway

import io.adb.catalog.Catalog
import io.adb.ffm.*
import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.{OptimizerConfig, RuleOptimizer}
import io.adb.optimizer.cardinality.{CardinalityEstimator, StatisticsStatus}
import io.adb.optimizer.cost.CostModel
import io.adb.optimizer.join.JoinReorderRule
import io.adb.physical.*
import io.adb.sql.SqlParser
import io.adb.statistics.{StatisticsCodec, StatisticsProvider}
import java.util.Optional
import scala.jdk.CollectionConverters.*

/** SQL front door of the JVM control plane:
  *
  * {{{
  * SQL -> SqlParser -> Binder/SelectBinder -> LogicalPlanner -> RuleOptimizer
  *     -> [cost mode] JoinReorderRule -> PhysicalPlanner (+ CostBasedPolicy | DefaultPlanningPolicy)
  *     -> PlanJsonEncoder -> Java FFM -> Rust
  * }}}
  *
  * DDL goes to the catalog, single-row DML to the native row API, `ANALYZE` to the engine's
  * statistics collector, queries through planning. In the default cost mode the planner uses
  * the engine's statistics (estimates, join order, build sides); `SET optimizer = rule`
  * switches to the rule-based planning of 2.1.3 (estimates are still shown by EXPLAIN).
  *
  * @param catalog     schema registry used for binding and DDL
  * @param native      open native database handle
  * @param config      cost-based optimizer configuration
  * @param feedbackLog where executed queries' estimated and actual rows are logged, if anywhere
  */
final class AdaptiveDatabase(
    catalog: Catalog,
    native: NativeDatabase,
    config: OptimizerConfig = OptimizerConfig.Default,
    feedbackLog: Option[PlannerFeedbackLog] = None
):
  /** SQL text to AST parser. */
  private val parser = new SqlParser
  /** Resolves names and types against the catalog. */
  private val binder = new Binder(catalog)
  /** Logical-plan rewriter. */
  private val optimizer = new RuleOptimizer()
  /** Statistics of the native engine. */
  private val statistics = EngineStatisticsProvider(native)
  /** Optimizer mode of this session (`SET optimizer = cost | rule`). */
  @volatile private var mode: OptimizerMode = OptimizerMode.Cost

  /** The session's optimizer mode. */
  def optimizerMode: OptimizerMode = mode

  /** Executes one SQL statement.
    *
    * @param sql statement text
    * @return rows for queries, plan text for EXPLAIN, or a status message for DDL/DML
    * @throws NativeException if the engine rejects the operation
    */
  def execute(sql: String): QueryResult =
    val ast = parser.parse(sql)
    val bound = binder.bind(ast)
    executeBound(bound, sql)

  /** Plans a bound SELECT without executing it (used by EXPLAIN, tests and benchmarks). */
  def plan(select: BoundSelect): AdaptiveDatabase.Plans = AdaptiveDatabase.plan(select, catalog, statistics, config, mode)

  /** Dispatches a bound statement: DDL goes to the catalog, DML to the native row API, SELECT and EXPLAIN through planning. */
  private def executeBound(statement: BoundStatement, sql: String): QueryResult = statement match
    case BoundCreateTable(name, fields, pk, references) =>
      val entity = catalog.createEntity(name, fields, pk, references)
      QueryResult(Vector.empty, Vector.empty, Some(s"created table ${entity.name} entityId=${entity.id.value}"))

    case BoundAnalyze(entities) =>
      val lines = entities.map { entity =>
        val table = StatisticsCodec.decode(native.analyzeJson(entity.id.value, null))
        statistics.invalidate(entity.id)
        s"ANALYZE ${entity.name} rows=${table.rowCount} sampled=${table.sampledRows} columns=${table.columns.size}"
      }
      QueryResult(Vector.empty, Vector.empty, Some(if lines.isEmpty then "ANALYZE (no tables)" else lines.mkString("\n")))

    case BoundSetOptimizer(newMode) =>
      mode = newMode
      QueryResult(Vector.empty, Vector.empty, Some(s"SET optimizer = ${newMode.toString.toLowerCase}"))

    case BoundInsert(entity, values) =>
      val pk = values.get(entity.primaryKeyField.id) match
        case Some(DbValue.Int64Value(value)) => value
        case _ => throw new IllegalArgumentException("BIGINT primary key value required")
      val ts = native.insert(entity.id.value, pk, MutationJsonEncoder.row(entity, values))
      QueryResult(Vector.empty, Vector.empty, Some(s"INSERT 1 commitTs=$ts"))

    case BoundUpdate(entity, assignments, pk) =>
      val ts = native.update(entity.id.value, pk, MutationJsonEncoder.assignments(assignments))
      QueryResult(Vector.empty, Vector.empty, Some(s"UPDATE 1 commitTs=$ts"))

    case BoundDelete(entity, pk) =>
      val ts = native.delete(entity.id.value, pk)
      QueryResult(Vector.empty, Vector.empty, Some(s"DELETE 1 commitTs=$ts"))

    case select: BoundSelect => run(select, plan(select), sql)._1

    case BoundExplain(inner: BoundSelect, analyze) =>
      val plans = plan(inner)
      val analyzed =
        if analyze then
          val (result, profile) = run(inner, plans, sql)
          Vector(s"Executed rows=${result.rows.size}", "Runtime profile:\n" + ProfileRenderer.render(profile, plans.physical.estimates))
        else Vector.empty
      QueryResult(Vector("plan"), (AdaptiveDatabase.explain(plans) ++ analyzed).map(section => Vector[Any](section)))

    case BoundExplain(other, _) =>
      QueryResult(Vector("plan"), Vector(Vector(other.toString)))

  /** Executes a planned SELECT: sends the physical plan to the engine as JSON (optionally at an
    * `AS OF` snapshot), materializes the output columns of every batch by slot, records the
    * estimate feedback, and returns the rows together with the native runtime profile.
    *
    * @throws IllegalStateException if the result exceeds [[AdaptiveDatabase.MaxMaterializedResultRows]]
    */
  private def run(select: BoundSelect, plans: AdaptiveDatabase.Plans, sql: String): (QueryResult, String) =
    val json = PlanJsonEncoder.encode(plans.physical.plan)
    val snapshot: Optional[java.lang.Long] = select.asOfVersion match
      case Some(v) => Optional.of(java.lang.Long.valueOf(v))
      case None => Optional.empty[java.lang.Long]()
    val query = native.execute(json, snapshot)
    try
      val rows = Vector.newBuilder[Vector[Any]]
      var materializedRows = 0
      var next = query.nextBatch()
      while next.isPresent do
        val batch = next.get()
        val bySlot = batch.columns().asScala.map(c => c.slotId() -> c).toMap
        val columns = select.output.map(column => bySlot.get(column.attribute.slot.value))
        for row <- 0 until batch.rowCount() do
          if materializedRows >= AdaptiveDatabase.MaxMaterializedResultRows then
            throw new IllegalStateException(s"result exceeds ${AdaptiveDatabase.MaxMaterializedResultRows} rows; use LIMIT")
          rows += columns.map(_.fold(null: Any)(_.values().get(row)))
          materializedRows += 1
        next = query.nextBatch()
      val profile = query.profileJson()
      feedbackLog.foreach(_.record(sql, plans.mode.toString.toLowerCase, ProfileRenderer.compare(profile, plans.physical.estimates)))
      (QueryResult(select.output.map(_.name), rows.result()), profile)
    finally query.close()

/** Planning pipeline, EXPLAIN rendering and limits of the gateway. */
object AdaptiveDatabase:
  /** Maximum rows materialized by one gateway query result. */
  val MaxMaterializedResultRows: Int = 1000000

  /** The planning stages of one SELECT.
    *
    * @param logical    canonical plan built from the bound statement
    * @param optimized  plan after the rule optimizer and, in cost mode, join ordering
    * @param physical   executable plan with decisions, estimates and warnings
    * @param mode       optimizer mode used
    * @param statistics statistics status of every entity the query reads, in plan order
    */
  final case class Plans(
      logical: LogicalPlan,
      optimized: LogicalPlan,
      physical: PlannedQuery,
      mode: OptimizerMode,
      statistics: Vector[(Entity, StatisticsStatus)]
  )

  /** Plans `select`: rule rewrites, then (cost mode) join ordering and cost-based strategies,
    * or (rule mode) the 2.1.3 rules; both annotate the plan with estimates.
    */
  def plan(select: BoundSelect, catalog: Catalog, statistics: StatisticsProvider, config: OptimizerConfig, mode: OptimizerMode): Plans =
    val estimator = new CardinalityEstimator(statistics, catalog, config.estimation)
    val costModel = new CostModel(estimator, config)
    val logical = LogicalPlanner.plan(select)
    val rewritten = new RuleOptimizer().optimize(logical)
    val (optimized, notes) =
      if mode == OptimizerMode.Cost then new JoinReorderRule(estimator, costModel).reorder(rewritten)
      else (rewritten, Vector.empty)
    val policy = if mode == OptimizerMode.Cost then new CostBasedPolicy(estimator, costModel) else DefaultPlanningPolicy
    val physical = PhysicalPlanner.plan(optimized, policy, Some(PlanEstimation(estimator, costModel)))
    val orderDecisions = notes.map(note => PlanDecision("join order", note.order, f"${note.method}; est. rows=${note.rows}%.0f cost=${note.cost}%.1f"))
    val relations = PhysicalPlanner.relations(optimized).distinctBy(_.entity.id)
    Plans(logical, optimized, physical.copy(decisions = orderDecisions ++ physical.decisions), mode, relations.map(r => r.entity -> estimator.status(r)))

  /** EXPLAIN sections of a plan: logical, optimized and physical plans (with estimates),
    * decisions, statistics and warnings.
    */
  def explain(plans: Plans): Vector[String] =
    val physical = plans.physical
    val statisticsLines = plans.statistics.map { (entity, status) =>
      status match
        case StatisticsStatus.Missing => s"- ${entity.name}: no statistics (run ANALYZE ${entity.name})"
        case StatisticsStatus.Fresh(s) =>
          s"- ${entity.name}: ${s.table.rowCount} rows analyzed at ts=${s.table.analyzedAtTs}, ${s.modificationsSinceAnalyze} modifications since (fresh)"
        case StatisticsStatus.Stale(s) =>
          s"- ${entity.name}: ${s.table.rowCount} rows analyzed at ts=${s.table.analyzedAtTs}, ${s.modificationsSinceAnalyze} modifications since (stale)"
    }
    Vector(
      "Logical:\n" + LogicalPlan.render(plans.logical),
      "Optimized:\n" + LogicalPlan.render(plans.optimized),
      s"Physical (optimizer=${plans.mode.toString.toLowerCase}):\n" + Explain.physical(physical.plan, physical.slotNames, physical.estimates),
      "Decisions:\n" + (if physical.decisions.isEmpty then "(none)" else physical.decisions.map("- " + _.display).mkString("\n")),
      "Statistics:\n" + (if statisticsLines.isEmpty then "(no tables)" else statisticsLines.mkString("\n"))
    ) ++ Option.when(physical.warnings.nonEmpty)("Warnings:\n" + physical.warnings.map("- " + _).mkString("\n"))
