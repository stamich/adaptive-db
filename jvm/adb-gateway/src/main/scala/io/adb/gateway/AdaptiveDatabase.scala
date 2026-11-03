package io.adb.gateway

import io.adb.consistency.*
import io.adb.optimizer.access.{IndexDecision, SemanticFreshnessPlanner}
import io.adb.plancache.*
import io.adb.advisor.{Advisor, AdvisorReport, AdvisorScheduler, EntityDesign, PhysicalDesign}
import io.adb.telemetry.*
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
import io.adb.statistics.{IndexStatistics, SchemaStatistics, StatisticsCodec, StatisticsProvider}
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
    feedbackLog: Option[PlannerFeedbackLog] = None,
    consistency: ConsistencyBackend = null,
    planCacheBytes: Long = AdaptiveDatabase.DefaultPlanCacheBytes
):
  /** The engine's commit order and projections (the native database unless a test supplies one). */
  private val backend: ConsistencyBackend = if consistency == null then ConsistencyBackend(native) else consistency
  /** The session behind [[execute]]: statements run without an explicit session share it. */
  private val defaultSession = new AdaptiveSession(this, backend)
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
  /** Planned SELECTs by statement; empty and unused when `planCacheBytes` is 0. */
  private val planCache = new PlanCache[CachedQuery](math.max(1L, planCacheBytes), 8, 4096, _.weight)
  /** Query telemetry (off until `SET telemetry = sampled | full`); see [[io.adb.telemetry.TelemetryRuntime]]. */
  val telemetry: TelemetryRuntime = new TelemetryRuntime()
  /** The workload advisor (2.5.3): reads telemetry snapshots and recommends; it never acts. */
  val advisor: Advisor = Advisor.forTelemetry(telemetry)
  /** The advisor's timer; created on the first `SET advisor = on`. */
  private var advisorTimer: Option[AdvisorScheduler] = None
  /** Whether the plan cache is consulted (`SET plan_cache = on | off`; off if the budget is 0). */
  @volatile private var cacheEnabled: Boolean = planCacheBytes > 0
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
  def execute(sql: String): QueryResult = executeIn(sql, defaultSession)

  /** A new session with its own consistency level and token, starting at the default level
    * (read your writes) and no writes.
    */
  def session(): AdaptiveSession = new AdaptiveSession(this, backend)

  /** The session [[execute]] uses. */
  def currentSession: AdaptiveSession = defaultSession

  /** Executes one SQL statement on behalf of `session`: its writes move the session's token and
    * its reads honour the session's consistency level.
    */
  private[gateway] def executeIn(sql: String, session: AdaptiveSession): QueryResult =
    val key = cacheKey(sql)
    key.flatMap(k => cachedResult(k, sql, session)).getOrElse {
      val ast = parser.parse(sql)
      try executeOnce(ast, sql, session, key)
      catch
        case stale: NativeException if stale.status() == AdbStatus.SCHEMA_CHANGED =>
          // The catalog changed between binding and execution (the engine checks before running,
          // so nothing was returned yet): bind against the new schemas and plan again, once.
          executeOnce(ast, sql, session, key)
    }

  /** The cache key of `sql` if it may use the plan cache: the cache is on and the text starts with
    * `SELECT` (a leading comment opts a statement out, which is harmless).
    */
  private def cacheKey(sql: String): Option[StatementKey] =
    if cacheEnabled && sql.dropWhile(_.isWhitespace).regionMatches(true, 0, "select", 0, 6)
    then StatementKey.of(sql, mode.toString.toLowerCase)
    else None

  /** Answers `sql` from the plan cache, or `None` to take the full path.
    *
    * A cached plan is used only if the engine's schema epoch and the statistics generations of
    * the entities it reads are unchanged, and the session's token is not beyond the head (the full
    * path then reports that). If the engine nevertheless refuses the plan as outdated, the entry
    * is dropped and the full path re-plans.
    */
  private def cachedResult(key: StatementKey, sql: String, session: AdaptiveSession): Option[QueryResult] =
    planCache.lookup(key, native.currentSchemaEpoch(), native.statisticsGeneration) match
      case Lookup.Hit(cached) =>
        val context = session.readContext()
        if context.consistency != ReadConsistency.Eventual && context.tokenIsAhead then None
        else
          val t0 = if telemetry.enabled then System.nanoTime() else 0L
          try Some(runCached(cached, sql, t0)._1)
          catch
            case stale: NativeException if stale.status() == AdbStatus.SCHEMA_CHANGED =>
              planCache.invalidate(key)
              None
            case NonFatal(e) =>
              if t0 != 0L then telemetryFailure(cached.shape, cached.schemaEpoch, cached.estimatedRows, cacheHit = true, t0)
              throw e
      case _ => None

  /** The plan-cache counters (the shell's `\\cache`). */
  def planCacheStats: CacheStats = planCache.stats

  /** The plan-cache counters as a result set of `(metric, value)`. */
  def showPlanCache(): QueryResult =
    val s = planCache.stats
    QueryResult(Vector("metric", "value"), Vector[(String, Any)](
      "enabled" -> cacheEnabled, "entries" -> s.entries, "bytes" -> s.bytes, "hits" -> s.hits,
      "misses" -> s.misses, "stale" -> s.stale, "inserts" -> s.inserts, "deferred" -> s.deferred,
      "rejected" -> s.rejected, "evictions" -> s.evictions, "hit_ratio" -> f"${s.hitRatio}%.3f"
    ).map((k, v) => Vector[Any](k, v)))

  /** Drops every cached plan; returns how many there were. */
  def clearPlanCache(): Int = planCache.clear()

  /** Every projection with its state and lag (the shell's `\\projections`). */
  def showProjections(): QueryResult =
    QueryResult(ConsistencyReport.ProjectionColumns, ConsistencyReport.projectionRows(backend.projections()))

  /** The level, token, head and wait limit of `session` (the shell's `\\session`). */
  def showSession(session: AdaptiveSession = defaultSession): QueryResult =
    QueryResult(ConsistencyReport.SessionColumns, ConsistencyReport.sessionRows(session.readContext()))

  /** The commits of the change log after `after`, at most `maxCommits` (the shell's `\\changes`).
    *
    * @throws NativeException if the change log no longer holds commits that old
    */
  def showChanges(after: Long, maxCommits: Int): QueryResult =
    require(maxCommits >= 1 && maxCommits <= 10000, "maxCommits must be 1..10000")
    val cursor = native.openChangeCursor(after)
    try
      val batch = cursor.nextBatch(maxCommits, AdaptiveDatabase.MaxChangePageBytes)
      QueryResult(ConsistencyReport.ChangeColumns, if batch.isPresent then ConsistencyReport.changeRows(batch.get().json()) else Vector.empty)
    finally cursor.close()

  /** Plans a bound SELECT without executing it (used by EXPLAIN, tests and benchmarks). */
  def plan(select: BoundSelect): AdaptiveDatabase.Plans = planAt(select, currentEpoch())

  /** Binds and executes one parsed statement at the current catalog epoch. */
  private def executeOnce(ast: io.adb.sql.Statement, sql: String, session: AdaptiveSession, key: Option[StatementKey]): QueryResult =
    val epoch = currentEpoch()
    val statement = binder.bind(ast)
    val result = executeBound(statement, sql, epoch, session, key)
    if AdaptiveDatabase.isDdl(statement) then
      // A schema change is a commit of its own; the head read now covers it (and, under
      // concurrency, possibly a few later commits, which only makes the token safer).
      val head = backend.latestCommitSeq()
      session.observe(head)
      result.copy(commitSeq = Some(head))
    else result

  /** The catalog epoch bound statements are planned with. It is read **before** the schema and
    * index metadata a plan is built from: if DDL commits in between, the plan carries the older
    * epoch and the engine refuses it instead of running it against changed tables.
    */
  private def currentEpoch(): Long = catalog match
    case versioned: SchemaCatalog => versioned.ensureCurrent().value
    case _ => native.currentSchemaEpoch()

  /** Plans a bound SELECT for execution at catalog epoch `epoch`. */
  private def planAt(select: BoundSelect, epoch: Long, indexes: IndexCatalog = indexCatalog): AdaptiveDatabase.Plans =
    AdaptiveDatabase.plan(select, catalog, statistics, config, mode, indexes).copy(schemaEpoch = epoch)

  /** Plans a SELECT for `session`: indexes whose projection may not serve the session's reads
    * are left out of planning, and the projections the chosen plan reads are waited for. If a
    * wait times out, the plan is made again without that index (reads fall back to the canonical
    * rows), so a lagging projection delays a read at most once, by the session's wait limit.
    *
    * @return the plan, and the read context it was made for
    * @throws IllegalStateException if the session's token is beyond the database's head
    */
  private def planFresh(select: BoundSelect, epoch: Long, session: AdaptiveSession): (AdaptiveDatabase.Plans, SemanticReadContext, Vector[IndexDecision]) =
    val context = session.readContext()
    if context.consistency != ReadConsistency.Eventual && context.tokenIsAhead then
      throw new IllegalStateException(
        s"session token ${context.token.commitSeq} is beyond this database's head ${context.headCommitSeq}; the token comes from another database or from a commit that was lost")
    val decisions = SemanticFreshnessPlanner.resolveIndexes(indexCatalog.indexes, backend.projections(), context)
    var allowed = decisions.filter(d => SemanticFreshnessPlanner.isUsable(d.resolution))
    var plans = planAt(select, epoch, IndexCatalog.of(allowed.map(_.index)*))
    val used = io.adb.physical.PhysicalPlan.preorder(plans.physical.plan).collect { case scan: io.adb.physical.PhysicalPlan.IndexScan => scan.indexId }.toSet
    val timedOut = SemanticFreshnessPlanner.waitsFor(allowed, used).collect {
      case (decision, projectionId, seq) if !backend.awaitProjection(projectionId, seq, context.maxWaitMillis) => decision
    }
    if timedOut.nonEmpty then
      allowed = allowed.filterNot(d => timedOut.exists(_.index.id == d.index.id))
      plans = planAt(select, epoch, IndexCatalog.of(allowed.map(_.index)*))
    (plans, context, decisions)

  /** Dispatches a bound statement: DDL goes to the catalog, DML to the native row API, SELECT and EXPLAIN through planning. */
  private def executeBound(statement: BoundStatement, sql: String, epoch: Long, session: AdaptiveSession, key: Option[StatementKey] = None): QueryResult = statement match
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

    case BoundSetPlanCache(enabled) =>
      cacheEnabled = enabled && planCacheBytes > 0
      QueryResult(Vector.empty, Vector.empty, Some(s"SET plan_cache = ${if cacheEnabled then "on" else "off"}"))

    case BoundSetTelemetry(name) =>
      telemetry.mode = name match
        case "off" => TelemetryMode.Off
        case "sampled" => TelemetryMode.Sampled
        case _ => TelemetryMode.Full
      QueryResult(Vector.empty, Vector.empty, Some(s"SET telemetry = $name"))

    case BoundSetAdvisor(enabled) =>
      if enabled then startAdvisor() else stopAdvisor()
      QueryResult(Vector.empty, Vector.empty, Some(s"SET advisor = ${if enabled then "on" else "off"}"))

    case BoundSetConsistency(level) =>
      session.consistency = level
      QueryResult(Vector.empty, Vector.empty, Some(s"SET SESSION CONSISTENCY ${level.sql}"))

    case BoundInsert(entity, values) =>
      val pk = values.get(entity.primaryKeyField.id) match
        case Some(DbValue.Int64Value(value)) => value
        case _ => throw new IllegalArgumentException("BIGINT primary key value required")
      val t0 = writeClock()
      val commit = native.insertCommit(entity.id.value, pk, MutationJsonEncoder.row(entity, values))
      recordWrite(entity.id.value, t0)
      committed("INSERT 1", commit, session)

    case BoundUpdate(entity, assignments, pk) =>
      val t0 = writeClock()
      val commit = native.updateCommit(entity.id.value, pk, MutationJsonEncoder.assignments(assignments))
      recordWrite(entity.id.value, t0)
      committed("UPDATE 1", commit, session)

    case BoundDelete(entity, pk) =>
      val t0 = writeClock()
      val commit = native.deleteCommit(entity.id.value, pk)
      recordWrite(entity.id.value, t0)
      committed("DELETE 1", commit, session)

    case select: BoundSelect =>
      // Generations are read before planning, so a change during planning leaves the stamp older
      // than the world and the entry is refused on its first lookup.
      val stamp = key.filter(_ => select.asOfVersion.isEmpty).map(_ => stampFor(select, epoch))
      val t0 = if telemetry.enabled then System.nanoTime() else 0L
      val (plans, _, decisions) = planFresh(select, epoch, session)
      val (result, _) =
        try run(select, plans, sql, t0, needProfile = false)
        catch
          case NonFatal(e) =>
            if t0 != 0L && !isSchemaChanged(e) then
              telemetryFailure(QueryShapeExtractor.extract(select), epoch, plans.physical.estimates.get(0).fold(-1L)(x => math.round(x.rows)), cacheHit = false, t0)
            throw e
      for k <- key; s <- stamp if cacheable(plans, decisions) do
        planCache.offer(k, CachedQuery.of(select, plans), s)
      result

    case BoundExplain(inner: BoundSelect, analyze) =>
      val (plans, context, decisions) = planFresh(inner, epoch, session)
      val analyzed =
        if analyze then
          val (result, profile) = run(inner, plans, sql)
          Vector(s"Executed rows=${result.rows.size}", "Runtime profile:\n" + ProfileRenderer.render(profile, plans.physical.estimates))
        else Vector.empty
      val sections = (AdaptiveDatabase.explain(plans) :+ ConsistencyReport.explain(context, decisions)) :+ explainCache(inner, plans, decisions)
      QueryResult(Vector("plan"), (sections ++ analyzed).map(section => Vector[Any](section)))

    case BoundExplain(other, _) =>
      QueryResult(Vector("plan"), Vector(Vector(other.toString)))

  /** The plan-cache section of EXPLAIN. EXPLAIN itself never reads or fills the cache (and
    * EXPLAIN ANALYZE always plans afresh); the section says whether this SELECT, run on its own,
    * would be cached and how the cache is doing.
    */
  private def explainCache(select: BoundSelect, plans: AdaptiveDatabase.Plans, decisions: Vector[IndexDecision]): String =
    val s = planCache.stats
    val verdict =
      if !cacheEnabled then "plan cache is off"
      else if select.asOfVersion.nonEmpty then "not cached: AS OF reads a snapshot"
      else if !cacheable(plans, decisions) then "not cached: the plan reads a projection that depends on the session's freshness"
      else "cacheable: stored on the second execution, valid until DDL or ANALYZE on a table it reads"
    s"Plan cache: $verdict (entries=${s.entries}, hits=${s.hits}, misses=${s.misses}, stale=${s.stale})"

  /** Whether `e` is the engine's "catalog moved" refusal, which is retried, not a failure. */
  private def isSchemaChanged(e: Throwable): Boolean = e match
    case n: NativeException => n.status() == AdbStatus.SCHEMA_CHANGED
    case _ => false

  /** Offers the event of a failed statement if the sampler wants it (failures are always captured
    * by the default policy). Never throws.
    */
  private def telemetryFailure(shape: QueryShape, epoch: Long, estimated: Long, cacheHit: Boolean, t0: Long): Unit =
    try
      val elapsed = System.nanoTime() - t0
      if telemetry.decide(shape.id, elapsed, failed = true) == SamplingDecision.Capture then
        telemetry.registry.register(shape)
        telemetry.offer(QueryTelemetry(shape.id, epoch, estimated, 0L, -1L, elapsed, cacheHit, failed = true))
    catch case NonFatal(_) => ()

  /** Start time of a write statement, or 0 when telemetry is off (so a write costs nothing extra). */
  private def writeClock(): Long = if telemetry.enabled then System.nanoTime() else 0L

  /** Counts one finished single-row write into the per-entity write counters (2.5.4).
    *
    * @param entityId the entity written
    * @param t0       the value [[writeClock]] returned before the statement; 0 means "not measured"
    */
  private def recordWrite(entityId: Long, t0: Long): Unit =
    if t0 != 0L then telemetry.writes.record(entityId, 1L, System.nanoTime() - t0)

  /** The telemetry counters as `(metric, value)` rows (the shell's `\\telemetry`). */
  def showTelemetry(): QueryResult =
    val m = telemetry.metrics
    QueryResult(Vector("metric", "value"), Vector[(String, Any)](
      "mode" -> m.mode.toString.toLowerCase, "skipped" -> m.skipped, "captured" -> m.captured,
      "accepted" -> m.accepted, "dropped" -> m.dropped, "drained" -> m.drained,
      "queue_size" -> m.queueSize, "queue_high_watermark" -> m.queueHighWatermark, "shapes" -> m.shapes
    ).map((k, v) => Vector[Any](k, v)))

  /** The busiest captured shapes (`\\telemetry shapes`): id, executions, mean and max time, mean
    * rows returned and read, worst estimate error. Folds the waiting events in first.
    */
  def showTelemetryShapes(limit: Int = 20): QueryResult =
    telemetry.drainNow()
    QueryResult(
      Vector("shape", "executions", "cache_hits", "failures", "mean_us", "max_us", "rows_returned", "rows_read", "max_q_error"),
      telemetry.allStats().take(limit).map { s =>
        val n = math.max(1L, s.executions)
        Vector[Any](java.lang.Long.toHexString(s.shapeId), s.executions, s.cacheHits, s.failures,
          f"${s.meanNanos / 1000.0}%.1f", s.maxNanos / 1000, s.totalActualRows / n, s.totalRowsRead / n, f"${s.maxQError}%.1f")
      })

  /** The physical design the advisor judges: names, row counts, statistics and indexes, read from
    * the catalog and the engine now and returned as immutable values.
    */
  def physicalDesign(): PhysicalDesign =
    PhysicalDesign.of(catalog.entities.map { entity =>
      val stats = statistics.statistics(entity.id)
      EntityDesign(
        entity.id.value,
        entity.name,
        entity.fields.map(f => f.id.value.toLong -> f.name).toMap,
        stats.fold(0L)(_.table.rowCount),
        stats,
        indexCatalog.indexes(entity.id)
      )
    })

  /** Starts evaluating the advisor in the background every `periodMillis` (`SET advisor = on`).
    * Turns nothing else on: without `SET telemetry = sampled | full` the advisor sees no queries.
    */
  def startAdvisor(periodMillis: Long = AdaptiveDatabase.AdvisorPeriodMillis): Unit = synchronized {
    if advisorTimer.isEmpty then advisorTimer = Some(new AdvisorScheduler(advisor, () => physicalDesign(), periodMillis))
    advisorTimer.foreach(_.start())
  }

  /** Stops the background evaluation (`SET advisor = off`); recommendations and the audit log stay. */
  def stopAdvisor(): Unit = synchronized(advisorTimer.foreach(_.stop()))

  /** Whether the advisor's timer runs. */
  def advisorRunning: Boolean = synchronized(advisorTimer.exists(_.running))

  /** The advisor's current advice (the shell's `\\advice`): `(kind, advice, sql)` rows.
    *
    * With the timer off, each call evaluates once (so the shell can be used by hand, and the
    * hysteresis counts calls); with the timer on, it shows the latest evaluation.
    */
  def showAdvice(): QueryResult =
    telemetry.drainNow()
    val report: AdvisorReport = if advisorRunning then advisor.lastReport.getOrElse(advisor.evaluate(physicalDesign())) else advisor.evaluate(physicalDesign())
    val rows = report.recommendations.map { r =>
      val kind = r match
        case _: io.adb.advisor.CreateIndex => "index"
        case _: io.adb.advisor.RunAnalyze => "analyze"
        case i: io.adb.advisor.Informational => i.kind
      Vector[Any](kind, r.summary, r.sql.getOrElse(""))
    }
    val note = if !telemetry.enabled then Some("telemetry is off: SET telemetry = sampled so the advisor sees queries") else None
    QueryResult(Vector("kind", "advice", "sql"), rows, note)

  /** The advisor's audit log (the shell's `\\advice log`): every confirmed, withdrawn and applied advice. */
  def showAdviceLog(): QueryResult =
    QueryResult(
      Vector("seq", "event", "advice", "sql"),
      advisor.auditLog.entries.map(e => Vector[Any](e.sequence, e.kind.label, e.summary, e.sql.getOrElse(""))))

  /** What a plan for `select` is valid for, read now. */
  private def stampFor(select: BoundSelect, epoch: Long): PlanStamp =
    PlanStamp(epoch, select.relations.map(_.entity.id.value).distinct.map(id => id -> native.statisticsGeneration(id)))

  /** Whether `plans` is valid for every session: it reads canonical rows and synchronous,
    * ready indexes only. A plan that reads an asynchronous or lagging projection depends on the
    * session's level and the projection's progress, so it is made again each time.
    */
  private def cacheable(plans: AdaptiveDatabase.Plans, decisions: Vector[IndexDecision]): Boolean =
    val used = io.adb.physical.PhysicalPlan.preorder(plans.physical.plan).collect { case scan: io.adb.physical.PhysicalPlan.IndexScan => scan.indexId }
    used.forall { id =>
      decisions.find(_.index.id == id).exists(d =>
        d.resolution == FreshnessResolution.Direct && d.projection.forall(_.mode == ProjectionMode.Synchronous))
    }

  /** The result of a DML statement; the session remembers its commit. */
  private def committed(verb: String, commit: NativeCommitResult, session: AdaptiveSession): QueryResult =
    session.observe(commit.commitSeq())
    QueryResult(Vector.empty, Vector.empty, Some(s"$verb commitTs=${commit.commitTs()} commitSeq=${commit.commitSeq()}"), Some(commit.commitSeq()))

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
  private def run(select: BoundSelect, plans: AdaptiveDatabase.Plans, sql: String, t0: Long = 0L, needProfile: Boolean = true): (QueryResult, String) =
    val shape = if t0 != 0L then QueryShapeExtractor.extract(select) else null
    runJson(
      PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch), select.asOfVersion, CachedQuery.outputs(select),
      plans.mode, plans.physical.estimates, sql, needProfile, t0, shape, cacheHit = false, plans.schemaEpoch,
      plans.physical.estimates.get(0).fold(-1L)(x => math.round(x.rows)))

  /** Executes a cached plan (present snapshot only; AS OF statements are not cached). */
  private def runCached(cached: CachedQuery, sql: String, t0: Long): (QueryResult, String) =
    runJson(cached.json, None, cached.outputs, cached.mode, cached.estimates, sql, false, t0, if t0 != 0L then cached.shape else null,
      cacheHit = true, cached.schemaEpoch, cached.estimatedRows)

  /** Sends encoded plan `json` to the engine and materializes `outputs` (name, slot) of every batch. */
  private def runJson(
      json: String,
      asOf: Option[Long],
      outputs: Vector[(String, Int)],
      planMode: OptimizerMode,
      estimates: Map[Int, io.adb.physical.NodeEstimate],
      sql: String,
      needProfile: Boolean,
      t0: Long,
      shape: QueryShape,
      cacheHit: Boolean,
      schemaEpoch: Long,
      estimatedRows: Long
  ): (QueryResult, String) =
    val snapshot: Optional[java.lang.Long] = asOf match
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
        val columns = outputs.map((_, slot) => bySlot.get(slot))
        for row <- 0 until batch.rowCount() do
          if materializedRows >= AdaptiveDatabase.MaxMaterializedResultRows then
            throw new IllegalStateException(s"result exceeds ${AdaptiveDatabase.MaxMaterializedResultRows} rows; use LIMIT")
          rows += columns.map(_.fold(null: Any)(_.values().get(row)))
          materializedRows += 1
        next = query.nextBatch()
      // Telemetry (2.5.2): `shape` is null when it is off, so the whole block is skipped. The
      // sampler sees only values at hand; an event is built for captured queries alone. Since
      // 2.5.2.1 a captured query reads its counters from the engine as four integers
      // (`readTelemetry`) instead of fetching, decoding and scanning the JSON profile; that
      // document is fetched only when EXPLAIN ANALYZE or the feedback log needs it.
      val elapsed = if shape != null then System.nanoTime() - t0 else 0L
      val captured = shape != null && telemetry.decide(shape.id, elapsed, failed = false) == SamplingDecision.Capture
      val profile = if needProfile || feedbackLog.isDefined then query.profileJson() else ""
      // Feedback must never fail a query that already ran.
      feedbackLog.foreach { log =>
        try log.record(sql, planMode.toString.toLowerCase, ProfileRenderer.compare(profile, estimates))
        catch case NonFatal(_) => ()
      }
      if captured then
        val scanned =
          try
            val counters = AdaptiveDatabase.telemetryBuffer.get()
            query.readTelemetry(counters)
            counters.rowsRead()
          catch case NonFatal(_) => -1L
        telemetry.registry.register(shape)
        telemetry.offer(QueryTelemetry(shape.id, schemaEpoch, estimatedRows, materializedRows, scanned, elapsed, cacheHit, failed = false))
      (QueryResult(outputs.map(_._1), rows.result()), profile)
    finally query.close()

/** Planning pipeline, EXPLAIN rendering and limits of the gateway. */
object AdaptiveDatabase:
  /** Time between the advisor's background evaluations after `SET advisor = on` (10 s). */
  val AdvisorPeriodMillis: Long = 10_000L

  /** Whether the statement is a schema change, which commits to the catalog. */
  def isDdl(statement: BoundStatement): Boolean = statement match
    case _: BoundCreateTable | _: BoundAlterTable | _: BoundDropTable | _: BoundCreateIndex | _: BoundDropIndex => true
    case _ => false

  /** One reusable native counter holder per thread (Milestone 2.5.2.1).
    *
    * A captured query reads its counters into this holder, so the read allocates neither a
    * profile document nor a native segment. It is per thread because the holder is not thread safe.
    */
  private val telemetryBuffer: ThreadLocal[io.adb.ffm.NativeQueryTelemetry] =
    ThreadLocal.withInitial[io.adb.ffm.NativeQueryTelemetry](() => new io.adb.ffm.NativeQueryTelemetry)

  /** Default byte budget of the plan cache: 32 MiB. */
  val DefaultPlanCacheBytes: Long = 32L << 20

  /** Largest page of change-log commits `showChanges` asks the engine for: 1 MiB. */
  val MaxChangePageBytes: Long = 1L << 20

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
    * @param schema     per table read: its schema version and the columns added after its last
    *                   `ANALYZE`, whose estimates are derived from their defaults
    */
  final case class Plans(
      logical: LogicalPlan,
      optimized: LogicalPlan,
      physical: PlannedQuery,
      mode: OptimizerMode,
      statistics: Vector[(Entity, StatisticsStatus)],
      indexes: Vector[IndexReport] = Vector.empty,
      schemaEpoch: Long = 0,
      schema: Vector[(Entity, Vector[SchemaStatistics.AddedAfterAnalyze])] = Vector.empty
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
    * both annotate the plan with estimates. Statistics are seen through the current schema:
    * columns added after `ANALYZE` get statistics derived from their defaults.
    */
  def plan(
      select: BoundSelect,
      catalog: Catalog,
      statistics: StatisticsProvider,
      config: OptimizerConfig,
      mode: OptimizerMode,
      indexes: IndexCatalog = IndexCatalog.Empty
  ): Plans =
    val schemaAware = SchemaStatistics.provider(statistics, catalog.entity)
    val estimator = new CardinalityEstimator(schemaAware, catalog, config.estimation)
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
      reports,
      schema = relations.map(r =>
        r.entity -> statistics.statistics(r.entity.id).fold(Vector.empty)(s => SchemaStatistics.addedAfterAnalyze(s.table, r.entity))
      )
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
    val schemaLines = plans.schema.flatMap { (entity, added) =>
      s"- ${entity.name}: schema version ${entity.schemaVersion.value}" +: added.map("  - " + _.display)
    }
    val schemaSection = Option.when(plans.schemaEpoch > 0 || plans.schema.exists(_._2.nonEmpty))(
      s"Schema (catalog epoch ${plans.schemaEpoch}):\n" + (if schemaLines.isEmpty then "(no tables)" else schemaLines.mkString("\n"))
    )
    Vector(
      "Logical:\n" + LogicalPlan.render(plans.logical),
      "Optimized:\n" + LogicalPlan.render(plans.optimized),
      s"Physical (optimizer=${plans.mode.toString.toLowerCase}):\n" + Explain.physical(physical.plan, physical.slotNames, physical.estimates, labels),
      "Decisions:\n" + (if physical.decisions.isEmpty then "(none)" else physical.decisions.map("- " + _.display).mkString("\n")),
      "Statistics:\n" + (if statisticsLines.isEmpty then "(no tables)" else statisticsLines.mkString("\n"))
    ) ++ schemaSection ++ Option.when(plans.indexes.nonEmpty)("Indexes:\n" + plans.indexes.map(_.display).mkString("\n")) ++
      Option.when(physical.warnings.nonEmpty)("Warnings:\n" + physical.warnings.map("- " + _).mkString("\n"))
