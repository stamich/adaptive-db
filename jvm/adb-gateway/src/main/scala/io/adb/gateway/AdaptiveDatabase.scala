package io.adb.gateway

import io.adb.catalog.{CatalogImport, Catalog, EngineCatalog, FileCatalog, IndexCatalog, IndexInfo, SchemaCatalog}
import io.adb.ffm.*
import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.{OptimizerConfig, RuleOptimizer}
import io.adb.optimizer.cardinality.{CardinalityEstimator, StatisticsStatus}
import io.adb.optimizer.cost.CostModel
import io.adb.optimizer.join.JoinReorderRule
import io.adb.physical.*
import io.adb.schema.{EntitySpec, IndexDependency, NewField, SchemaHistory}
import java.nio.file.{Files, Path, StandardCopyOption}
import io.adb.sql.SqlParser
import io.adb.statistics.{IndexStatistics, StatisticsCodec, StatisticsProvider}
import java.util.Optional
import scala.util.control.NonFatal
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
  * statistics collector, queries through planning. Since 2.4.0 the catalog is normally the
  * engine's own ([[AdaptiveDatabase.open]]): every statement first brings the cached schemas up
  * to the engine's catalog epoch, binds against them and plans with that epoch; a plan the
  * engine refuses as stale (`SCHEMA_CHANGED`, DDL by another client in between) is bound and
  * planned once more. In the default cost mode the planner uses
  * the engine's statistics (estimates, join order, build sides); `SET optimizer = rule`
  * switches to the rule-based planning of 2.1.3 (estimates are still shown by EXPLAIN).
  *
  * @param catalog     schema registry used for binding and DDL (an [[io.adb.catalog.EngineCatalog]],
  *                    or a fixed catalog for tools that manage schemas themselves)
  * @param native      open native database handle
  * @param config      cost-based optimizer configuration
  * @param feedbackLog where executed queries' estimated and actual rows are logged, if anywhere
  */
final class AdaptiveDatabase(
    val catalog: Catalog,
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
  /** Index definitions of the native engine. */
  private val indexCatalog = EngineIndexCatalog(native)
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
    try executeOnce(ast, sql)
    catch
      case stale: NativeException if stale.status() == AdbStatus.SCHEMA_CHANGED =>
        // The catalog changed between binding and execution (the engine checks before running,
        // so nothing was returned yet): bind against the new schemas and plan again, once.
        executeOnce(ast, sql)

  /** Plans a bound SELECT without executing it (used by EXPLAIN, tests and benchmarks). */
  def plan(select: BoundSelect): AdaptiveDatabase.Plans = planAt(select, currentEpoch())

  /** Binds and executes one parsed statement at the current catalog epoch. */
  private def executeOnce(ast: io.adb.sql.Statement, sql: String): QueryResult =
    val epoch = currentEpoch()
    executeBound(binder.bind(ast), sql, epoch)

  /** The catalog epoch bound statements are planned with. It is read **before** the schema and
    * index metadata a plan is built from: if DDL commits in between, the plan carries the older
    * epoch and the engine refuses it instead of running it against changed tables.
    */
  private def currentEpoch(): Long = catalog match
    case versioned: SchemaCatalog => versioned.ensureCurrent().value
    case _ => native.currentSchemaEpoch()

  /** Plans a bound SELECT for execution at catalog epoch `epoch`. */
  private def planAt(select: BoundSelect, epoch: Long): AdaptiveDatabase.Plans =
    AdaptiveDatabase.plan(select, catalog, statistics, config, mode, indexCatalog).copy(schemaEpoch = epoch)

  /** Dispatches a bound statement: DDL goes to the catalog, DML to the native row API, SELECT and EXPLAIN through planning. */
  private def executeBound(statement: BoundStatement, sql: String, epoch: Long): QueryResult = statement match
    case BoundCreateTable(name, fields, pk, references, defaults) =>
      val entity = catalog match
        case versioned: SchemaCatalog =>
          // The binder keys defaults and hints by the column name as declared.
          versioned.createEntity(
            EntitySpec(
              name,
              fields.map((column, dataType, nullable) => NewField(column, dataType, nullable, defaults.get(column), references.get(column))),
              pk
            )
          )
        case legacy =>
          require(defaults.isEmpty, "column defaults need the engine catalog")
          legacy.createEntity(name, fields, pk, references)
      QueryResult(Vector.empty, Vector.empty, Some(s"created table ${entity.name} entityId=${entity.id.value}"))

    case BoundAlterTable(entity, change) =>
      val altered = schemaCatalog("ALTER TABLE").alterEntity(entity.id, change)
      QueryResult(Vector.empty, Vector.empty, Some(s"altered table ${altered.name}: ${change.sql} (schema version ${altered.version.value})"))

    case BoundDropTable(entity) =>
      schemaCatalog("DROP TABLE").dropEntity(entity.id)
      QueryResult(Vector.empty, Vector.empty, Some(s"dropped table ${entity.name}"))

    case BoundShowSchema(entity) =>
      QueryResult(Vector("version", "epoch", "change", "columns"), AdaptiveDatabase.describe(schemaCatalog("SHOW SCHEMA").history(entity.id)))

    case BoundCreateIndex(name, entity, columns, include) =>
      val keys = columns.map((field, descending) =>
        s"{\"field_id\":${field.id.value},\"direction\":\"${if descending then "desc" else "asc"}\"}")
      val spec = s"{\"name\":${PlanJsonEncoder.quote(name)},\"entity_id\":${java.lang.Long.toUnsignedString(entity.id.value)}," +
        s"\"columns\":${keys.mkString("[", ",", "]")},\"include\":${include.map(_.id.value).mkString("[", ",", "]")}}"
      val created = IndexCatalog.decodeStatus(io.adb.model.JsonReader.parse(native.createIndexJson(spec)).asInstanceOf[Map[String, Any]])
      QueryResult(Vector.empty, Vector.empty, Some(s"created index ${created.name} on ${entity.name} id=${created.id.value} usable from ts=${created.usableFromTs}"))

    case BoundDropIndex(name) =>
      native.dropIndex(name)
      QueryResult(Vector.empty, Vector.empty, Some(s"dropped index $name"))

    case BoundShowIndexes(entity) =>
      val shown = entity.fold(indexCatalog.indexes)(e => indexCatalog.indexes(e.id))
      QueryResult(
        Vector("name", "table", "columns", "include", "usable_from_ts", "watermark_ts"),
        shown.map(index => AdaptiveDatabase.describe(index, catalog))
      )

    case BoundAnalyze(entities) =>
      val lines = entities.map { entity =>
        val table = StatisticsCodec.decode(native.analyzeJson(entity.id.value, null))
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

    case select: BoundSelect => run(select, planAt(select, epoch), sql)._1

    case BoundExplain(inner: BoundSelect, analyze) =>
      val plans = planAt(inner, epoch)
      val analyzed =
        if analyze then
          val (result, profile) = run(inner, plans, sql)
          Vector(s"Executed rows=${result.rows.size}", "Runtime profile:\n" + ProfileRenderer.render(profile, plans.physical.estimates))
        else Vector.empty
      QueryResult(Vector("plan"), (AdaptiveDatabase.explain(plans) ++ analyzed).map(section => Vector[Any](section)))

    case BoundExplain(other, _) =>
      QueryResult(Vector("plan"), Vector(Vector(other.toString)))

  /** The catalog as a versioned one, for statements that need schema evolution. */
  private def schemaCatalog(statement: String): SchemaCatalog = catalog match
    case versioned: SchemaCatalog => versioned
    case _ => throw new IllegalStateException(s"$statement needs the engine catalog (this session uses a fixed catalog)")

  /** Executes a planned SELECT: sends the physical plan to the engine as JSON (optionally at an
    * `AS OF` snapshot), materializes the output columns of every batch by slot, records the
    * estimate feedback, and returns the rows together with the native runtime profile.
    *
    * @throws IllegalStateException if the result exceeds [[AdaptiveDatabase.MaxMaterializedResultRows]]
    */
  private def run(select: BoundSelect, plans: AdaptiveDatabase.Plans, sql: String): (QueryResult, String) =
    val json = PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch)
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
      // Feedback must never fail a query that already ran.
      feedbackLog.foreach { log =>
        try log.record(sql, plans.mode.toString.toLowerCase, ProfileRenderer.compare(profile, plans.physical.estimates))
        catch case NonFatal(_) => ()
      }
      (QueryResult(select.output.map(_.name), rows.result()), profile)
    finally query.close()

/** Planning pipeline, EXPLAIN rendering and limits of the gateway. */
object AdaptiveDatabase:
  /** Maximum rows materialized by one gateway query result. */
  val MaxMaterializedResultRows: Int = 1000000

  /** Opens the SQL front door over an open native database with the engine's own catalog.
    *
    * A database written before Milestone 2.4.0 keeps its schemas in a JVM catalog file; if
    * `legacyCatalog` exists it is imported into the engine with the same entity and field ids
    * (see [[io.adb.catalog.CatalogImport]]) and then renamed to `<name>.imported`, so the import
    * happens once and the engine catalog is the only one afterwards.
    *
    * @param native        open native database
    * @param legacyCatalog the 2.0–2.3 `catalog.properties` of this database, if any
    * @param config        cost-based optimizer configuration
    * @param feedbackLog   where estimate feedback is logged, if anywhere
    */
  def open(
      native: NativeDatabase,
      legacyCatalog: Option[Path] = None,
      config: OptimizerConfig = OptimizerConfig.Default,
      feedbackLog: Option[PlannerFeedbackLog] = None
  ): AdaptiveDatabase =
    val indexes = EngineIndexCatalog(native)
    val catalog = new EngineCatalog(new NativeSchemaBackend(native), () => indexes.indexes.map(dependency))
    legacyCatalog.filter(Files.exists(_)).foreach { path =>
      CatalogImport.importInto(catalog, new FileCatalog(path))
      Files.move(path, path.resolveSibling(path.getFileName.toString + ".imported"), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    }
    new AdaptiveDatabase(catalog, native, config, feedbackLog)

  /** The fields an index depends on, for RESTRICT checks of `DROP COLUMN` and `DROP TABLE`. */
  def dependency(index: IndexInfo): IndexDependency =
    IndexDependency(index.id, index.name, index.entity, index.columns.map(_.field), index.include)

  /** One `SHOW INDEXES` row: name, table, key columns (`col [DESC]`), covered columns and the
    * range of snapshots the index serves.
    */
  def describe(index: IndexInfo, catalog: Catalog): Vector[Any] =
    val entity = catalog.entity(index.entity)
    /** The column name of `field`, or `#field` if the catalog does not know it. */
    def name(field: FieldId): String = entity.flatMap(_.fieldsById.get(field)).fold(s"#${field.value}")(_.name)
    Vector(
      index.name,
      entity.fold(s"#${index.entity.value}")(_.name),
      index.columns.map(c => name(c.field) + (if c.descending then " DESC" else "")).mkString(", "),
      index.include.map(name).mkString(", "),
      java.lang.Long.valueOf(index.usableFromTs),
      java.lang.Long.valueOf(index.watermarkTs)
    )

  /** `SHOW SCHEMA` rows, one per version: version, catalog epoch, the change that produced it
    * (`CREATE TABLE` for version 1) and the columns it has.
    */
  def describe(history: SchemaHistory): Vector[Vector[Any]] =
    val changes = history.changes.toMap
    history.versions.map { schema =>
      Vector[Any](
        java.lang.Long.valueOf(schema.version.value),
        java.lang.Long.valueOf(schema.epoch.value),
        changes.get(schema.version).fold("CREATE TABLE")(_.sql),
        schema.columnsSql
      )
    }

  /** The planning stages of one SELECT.
    *
    * @param logical    canonical plan built from the bound statement
    * @param optimized  plan after the rule optimizer and, in cost mode, join ordering
    * @param physical   executable plan with decisions, estimates and warnings
    * @param mode       optimizer mode used
    * @param statistics statistics status of every entity the query reads, in plan order
    * @param indexes    the indexes of those entities the planner considered (cost mode only)
    * @param schemaEpoch catalog epoch the plan was made at (sent with the plan; the engine
    *                    refuses it once the epoch has moved)
    */
  final case class Plans(
      logical: LogicalPlan,
      optimized: LogicalPlan,
      physical: PlannedQuery,
      mode: OptimizerMode,
      statistics: Vector[(Entity, StatisticsStatus)],
      indexes: Vector[IndexReport] = Vector.empty,
      schemaEpoch: Long = 0
  )

  /** What EXPLAIN reports about one index of an entity the query reads.
    *
    * @param info       the engine's definition and serving range
    * @param entity     the indexed entity
    * @param statistics the index statistics of the last `ANALYZE`, if any
    * @param serves     whether the index can answer the query snapshot
    */
  final case class IndexReport(info: IndexInfo, entity: Entity, statistics: Option[IndexStatistics], serves: Boolean):
    /** Key column names, most significant first (`#field` if the catalog does not know one). */
    def keyColumns: Vector[String] = info.columns.map(c => fieldName(c.field))

    /** `idx ON table (a, b DESC) INCLUDE (c)`. */
    def definition: String =
      val keys = info.columns.map(c => fieldName(c.field) + (if c.descending then " DESC" else "")).mkString(", ")
      val include = if info.include.isEmpty then "" else info.include.map(fieldName).mkString(" INCLUDE (", ", ", ")")
      s"${info.name} ON ${entity.name} ($keys)$include"

    /** The EXPLAIN line: definition, serving range and statistics status. */
    def display: String =
      val range =
        if serves then s"serves ts ${info.usableFromTs}..${java.lang.Long.toUnsignedString(info.watermarkTs)}"
        else s"cannot serve the query snapshot (serves ts ${info.usableFromTs}..${java.lang.Long.toUnsignedString(info.watermarkTs)})"
      val stats = statistics match
        case None => s"no statistics (run ANALYZE ${entity.name})"
        case Some(s) =>
          val ndv = s.distinctPrefixes.mkString("[", ", ", "]")
          f"${s.entries} entries (${s.storedEntries} stored), height ${s.height}, ${s.leafPages} leaf pages, prefix ndv $ndv, pk locality ${s.pkLocality}%.2f"
      s"- $definition: $range; $stats"

    /** The name of `field`. */
    private def fieldName(field: FieldId): String = entity.fieldsById.get(field).fold(s"#${field.value}")(_.name)

  /** Plans `select`: rule rewrites, then (cost mode) join ordering and cost-based strategies
    * including access paths over `indexes`, or (rule mode) the 2.1.3 rules without indexes;
    * both annotate the plan with estimates.
    */
  def plan(
      select: BoundSelect,
      catalog: Catalog,
      statistics: StatisticsProvider,
      config: OptimizerConfig,
      mode: OptimizerMode,
      indexes: IndexCatalog = IndexCatalog.Empty
  ): Plans =
    val estimator = new CardinalityEstimator(statistics, catalog, config.estimation)
    val costModel = new CostModel(estimator, config)
    val logical = LogicalPlanner.plan(select)
    val rewritten = new RuleOptimizer().optimize(logical)
    val (optimized, notes) =
      if mode == OptimizerMode.Cost then new JoinReorderRule(estimator, costModel).reorder(rewritten)
      else (rewritten, Vector.empty)
    val policy =
      if mode == OptimizerMode.Cost then new CostBasedPolicy(estimator, costModel, IndexCatalog.of(indexes.indexes*), select.asOfVersion)
      else DefaultPlanningPolicy
    val physical = PhysicalPlanner.plan(optimized, policy, Some(PlanEstimation(estimator, costModel)))
    val orderDecisions = notes.map(note => PlanDecision("join order", note.order, f"${note.method}; est. rows=${note.rows}%.0f cost=${note.cost}%.1f"))
    val relations = PhysicalPlanner.relations(optimized).distinctBy(_.entity.id)
    val reports =
      if mode != OptimizerMode.Cost then Vector.empty
      else
        val snapshot = select.asOfVersion
        relations.flatMap { relation =>
          indexes.indexes(relation.entity.id).map { info =>
            val stats = estimator.entityStatistics(relation.entity.id).flatMap(_.table.index(info.id))
            IndexReport(info, relation.entity, stats, snapshot.forall(info.serves))
          }
        }
    Plans(
      logical,
      optimized,
      physical.copy(decisions = orderDecisions ++ physical.decisions),
      mode,
      relations.map(r => r.entity -> estimator.status(r)),
      reports
    )

  /** EXPLAIN sections of a plan: logical, optimized and physical plans (with estimates and
    * index names), decisions, statistics, the indexes considered and warnings.
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
    val labels = plans.indexes.map(r => r.info.id -> IndexLabel(r.info.name, r.keyColumns)).toMap
    Vector(
      "Logical:\n" + LogicalPlan.render(plans.logical),
      "Optimized:\n" + LogicalPlan.render(plans.optimized),
      s"Physical (optimizer=${plans.mode.toString.toLowerCase}):\n" + Explain.physical(physical.plan, physical.slotNames, physical.estimates, labels),
      "Decisions:\n" + (if physical.decisions.isEmpty then "(none)" else physical.decisions.map("- " + _.display).mkString("\n")),
      "Statistics:\n" + (if statisticsLines.isEmpty then "(no tables)" else statisticsLines.mkString("\n"))
    ) ++ Option.when(plans.indexes.nonEmpty)("Indexes:\n" + plans.indexes.map(_.display).mkString("\n")) ++
      Option.when(physical.warnings.nonEmpty)("Warnings:\n" + physical.warnings.map("- " + _).mkString("\n"))
