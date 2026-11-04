package io.adb.benchmark

import io.adb.bench.*
import io.adb.benchmark.FamilyCommon.*
import io.adb.catalog.InMemoryCatalog
import io.adb.logical.{Binder, BoundSelect, LogicalPlanner}
import io.adb.optimizer.RuleOptimizer
import io.adb.physical.{PhysicalPlanner, PlanJsonEncoder}
import io.adb.plancache.StatementKey
import io.adb.sql.SqlParser

/** The `sql-path` family (Milestone 2.5.4.1): what a SQL statement costs before and around the
  * engine.
  *
  * Planner stages are timed for a point lookup and a relational query (parse, bind, logical plan,
  * rule optimiser, physical plan, plan JSON, and the whole pipeline); then the plan-cache key; then,
  * when a native library is given, the whole SQL path through Java FFM with the plan cache off and
  * on. The difference between the two is the planning cost the cache removes. The stage cases need no
  * engine and always run; the gateway cases are reported as skipped without one.
  */
object SqlPathFamily:
  /** Runs the family and returns the report JSON. */
  def run(settings: FamilySettings): String =
    val plan = cpuPlan
    val catalog = new InMemoryCatalog
    createTables(catalog)
    val planner = plannerCases("", PointSql, catalog, plan) ++ plannerCases("_relational", RelationalSql, catalog, plan)
    val keys = Vector(
      "plan_cache_key_point" -> timed(plan)(StatementKey.of(PointSql, "cost")),
      "plan_cache_key_relational" -> timed(plan)(StatementKey.of(RelationalSql, "cost"))
    )
    val gateway = (settings.dataDir, settings.nativeLib) match
      case (Some(data), Some(lib)) => gatewayCases(data, lib, plan)
      case _ => gatewayNames.map(_ -> skipped("no --data and --native-lib given"))
    finish("sql-path", plan, settings, Seq("setup" -> Json.obj("account_rows" -> "1000", "transfer_rows" -> "10000")), planner ++ keys ++ gateway)

  /** Names of the gateway cases, so a skipped run lists them. */
  private val gatewayNames = Vector(
    "gateway_point_lookup_cache_off", "gateway_point_lookup_cache_on", "gateway_join_aggregate_cache_off",
    "gateway_join_aggregate_cache_on", "gateway_point_lookup_sql_to_rust", "gateway_scan_filter_limit_sql_to_rust",
    "gateway_join_aggregate_top_k_sql_to_rust"
  )

  /** Times each planning stage of `sql`; stage names get `suffix`. */
  private def plannerCases(suffix: String, sql: String, catalog: InMemoryCatalog, plan: MeasurementPlan): Vector[(String, String)] =
    val parser = new SqlParser
    val binder = new Binder(catalog)
    val optimizer = new RuleOptimizer()
    val ast = parser.parse(sql)
    val bound = binder.bind(ast).asInstanceOf[BoundSelect]
    val logical = LogicalPlanner.plan(bound)
    val optimized = optimizer.optimize(logical)
    val physical = PhysicalPlanner.plan(optimized)
    def pipeline(): String =
      val b = binder.bind(parser.parse(sql)).asInstanceOf[BoundSelect]
      PlanJsonEncoder.encode(PhysicalPlanner.plan(optimizer.optimize(LogicalPlanner.plan(b))).plan, 0)
    Vector(
      s"sql_parse$suffix" -> timed(plan)(parser.parse(sql)),
      s"bind_slots$suffix" -> timed(plan)(binder.bind(ast)),
      s"logical_plan$suffix" -> timed(plan)(LogicalPlanner.plan(bound)),
      s"rule_optimize$suffix" -> timed(plan)(optimizer.optimize(logical)),
      s"physical_plan$suffix" -> timed(plan)(PhysicalPlanner.plan(optimized)),
      s"plan_json_encode$suffix" -> timed(plan)(PlanJsonEncoder.encode(physical.plan, 0)),
      s"planner_pipeline$suffix" -> timed(plan)(pipeline())
    )

  /** The SQL path through the native engine, plan cache off and on. */
  private def gatewayCases(data: java.nio.file.Path, lib: java.nio.file.Path, plan: MeasurementPlan): Vector[(String, String)] =
    val (native, db) = openGateway(data, lib)
    try
      val scan = "SELECT id, balance FROM account WHERE balance >= 9000 LIMIT 100;"
      // The statements as a user runs them (the default plan-cache setting), then with the cache off
      // and on explicitly; the three pairs are separate measurements.
      val pointDefault = timed(plan)(db.execute(GatewayPointSql))
      val scanDefault = timed(plan)(db.execute(scan))
      val relDefault = timed(plan)(db.execute(RelationalSql))
      db.execute("SET plan_cache = off")
      val pointOff = timed(plan)(db.execute(GatewayPointSql))
      val relOff = timed(plan)(db.execute(RelationalSql))
      db.execute("SET plan_cache = on")
      db.execute(GatewayPointSql); db.execute(RelationalSql); db.execute(RelationalSql)
      val pointOn = timed(plan)(db.execute(GatewayPointSql))
      val relOn = timed(plan)(db.execute(RelationalSql))
      Vector(
        "gateway_point_lookup_cache_off" -> pointOff,
        "gateway_point_lookup_cache_on" -> pointOn,
        "gateway_join_aggregate_cache_off" -> relOff,
        "gateway_join_aggregate_cache_on" -> relOn,
        "gateway_point_lookup_sql_to_rust" -> pointDefault,
        "gateway_scan_filter_limit_sql_to_rust" -> scanDefault,
        "gateway_join_aggregate_top_k_sql_to_rust" -> relDefault
      )
    finally native.close()
