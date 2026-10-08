package io.adb.gateway

import io.adb.catalog.Catalog
import io.adb.ffm.*
import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.RuleOptimizer
import io.adb.physical.*
import io.adb.sql.SqlParser
import java.util.Optional
import scala.jdk.CollectionConverters.*

/** SQL front door of the JVM control plane:
  *
  * {{{
  * SQL -> SqlParser -> Binder/SelectBinder -> LogicalPlanner -> RuleOptimizer
  *     -> PhysicalPlanner (+ PlanningPolicy) -> PlanJsonEncoder -> Java FFM -> Rust
  * }}}
  *
  * DDL goes to the catalog, single-row DML to the native row API, queries through planning.
  *
  * @param catalog schema registry used for binding and DDL
  * @param native  open native database handle
  * @param policy  physical strategy policy (the seam for statistics- or intent-driven planning)
  */
final class AdaptiveDatabase(catalog: Catalog, native: NativeDatabase, policy: PlanningPolicy = DefaultPlanningPolicy):
  /** SQL text to AST parser. */
  private val parser = new SqlParser
  /** Resolves names and types against the catalog. */
  private val binder = new Binder(catalog)
  /** Logical-plan rewriter. */
  private val optimizer = new RuleOptimizer()
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
    executeBound(bound)

  /** Plans a bound SELECT without executing it (used by EXPLAIN, tests and benchmarks). */
  def plan(select: BoundSelect): AdaptiveDatabase.Plans =
    val logical = LogicalPlanner.plan(select)
    val optimized = optimizer.optimize(logical)
    AdaptiveDatabase.Plans(logical, optimized, PhysicalPlanner.plan(optimized, policy))

  /** Dispatches a bound statement: DDL goes to the catalog, DML to the native row API, SELECT and EXPLAIN through planning. */
  private def executeBound(statement: BoundStatement): QueryResult = statement match
    case BoundCreateTable(name, fields, pk, references) =>
      val entity = catalog.createEntity(name, fields, pk, references)
      QueryResult(Vector.empty, Vector.empty, Some(s"created table ${entity.name} entityId=${entity.id.value}"))

    case BoundAnalyze(entities) =>
      val lines = entities.map { entity =>
        val statistics = io.adb.statistics.StatisticsCodec.decode(native.analyzeJson(entity.id.value, null))
        s"ANALYZE ${entity.name} rows=${statistics.rowCount} sampled=${statistics.sampledRows} columns=${statistics.columns.size}"
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

    case select: BoundSelect => run(select, plan(select))._1

    case BoundExplain(inner: BoundSelect, analyze) =>
      val plans = plan(inner)
      val sections = Vector(
        "Logical:\n" + LogicalPlan.render(plans.logical),
        "Optimized:\n" + LogicalPlan.render(plans.optimized),
        "Physical:\n" + Explain.physical(plans.physical.plan, plans.physical.slotNames),
        "Decisions:\n" + (if plans.physical.decisions.isEmpty then "(none)" else plans.physical.decisions.map("- " + _.display).mkString("\n"))
      )
      val analyzed =
        if analyze then
          val (result, profile) = run(inner, plans)
          Vector(s"Executed rows=${result.rows.size}", "Runtime profile:\n" + ProfileRenderer.render(profile))
        else Vector.empty
      QueryResult(Vector("plan"), (sections ++ analyzed).map(section => Vector[Any](section)))

    case BoundExplain(other, _) =>
      QueryResult(Vector("plan"), Vector(Vector(other.toString)))

  /** Executes a planned SELECT: sends the physical plan to the engine as JSON (optionally at an
    * `AS OF` snapshot), materializes the output columns of every batch by slot, and returns the
    * rows together with the native runtime profile.
    *
    * @throws IllegalStateException if the result exceeds [[AdaptiveDatabase.MaxMaterializedResultRows]]
    */
  private def run(select: BoundSelect, plans: AdaptiveDatabase.Plans): (QueryResult, String) =
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
      (QueryResult(select.output.map(_.name), rows.result()), query.profileJson())
    finally query.close()

/** Plans of one query and the gateway's limits. */
object AdaptiveDatabase:
  /** Maximum rows materialized by one gateway query result. */
  val MaxMaterializedResultRows: Int = 1000000

  /** The planning stages of one SELECT.
    *
    * @param logical   canonical plan built from the bound statement
    * @param optimized plan after the rule optimizer
    * @param physical  executable plan with decisions and slot names
    */
  final case class Plans(logical: LogicalPlan, optimized: LogicalPlan, physical: PlannedQuery)
