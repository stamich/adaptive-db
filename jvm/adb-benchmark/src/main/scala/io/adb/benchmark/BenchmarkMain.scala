package io.adb.benchmark

import io.adb.catalog.{Catalog, FileCatalog, InMemoryCatalog}
import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import io.adb.logical.{Binder, BoundCreateTable, BoundSelect, LogicalPlanner}
import io.adb.optimizer.RuleOptimizer
import io.adb.physical.{PhysicalPlanner, PlanJsonEncoder}
import io.adb.sql.SqlParser

import java.nio.file.{Files, Path}
import scala.annotation.tailrec

/** JVM benchmark (Milestone 2.2.3) separating JVM planning cost from native end-to-end execution.
  *
  * Planner stages are timed for a point lookup and for a relational query (join, GROUP BY,
  * ORDER BY + LIMIT); the optional gateway section runs both through Java FFM against the
  * native engine. `--workloads` runs the 2.2.3 workload benchmark instead
  * ([[WorkloadBenchmark]]).
  */
object BenchmarkMain:
  /** Command-line options.
    *
    * @param iterations        iterations of each planner micro-benchmark
    * @param gatewayIterations iterations of the SQL-to-native gateway benchmark
    * @param dataDir           database directory for the gateway benchmark
    * @param nativeLib         path of the native library for the gateway benchmark
    * @param workloads         run the 2.2.3 workload benchmark (needs `dataDir` and `nativeLib`)
    * @param out               where the workload benchmark writes its JSON report
    * @param scale             scale factor the workload data was prepared with
    * @param warmupMs          minimum warm-up time of each timed workload path
    */
  private final case class Config(
      iterations: Int = 10000,
      gatewayIterations: Int = 1000,
      dataDir: Option[Path] = None,
      nativeLib: Option[Path] = None,
      workloads: Boolean = false,
      out: Option[Path] = None,
      scale: Int = 1,
      warmupMs: Long = 2000
  )

  /** Point lookup used since 2.0. */
  private val PointSql = "SELECT id, owner, balance FROM account WHERE id = 42 LIMIT 10;"
  /** Relational query of 2.1: join, aggregate, ordered limit. */
  private val RelationalSql =
    "SELECT a.owner, COUNT(*) AS n, SUM(t.amount) AS total FROM account a JOIN transfer t ON t.account_id = a.id " +
      "WHERE t.amount > 0 GROUP BY a.owner ORDER BY total DESC LIMIT 10;"

  /** Runs the planner benchmark, then the gateway benchmark when both `--data` and `--native-lib` are given. */
  def main(args: Array[String]): Unit =
    val cfg = parseArgs(args.toList, Config())
    if cfg.workloads then
      (cfg.dataDir, cfg.nativeLib) match
        case (Some(data), Some(lib)) =>
          WorkloadBenchmark.run(WorkloadBenchmark.Settings(data, lib, cfg.iterations, cfg.scale, cfg.warmupMs, cfg.out))
        case _ => throw new IllegalArgumentException("--workloads needs --data DIR and --native-lib FILE")
      return
    println(s"Adaptive DB 2.2.3 JVM benchmark iterations=${cfg.iterations} gatewayIterations=${cfg.gatewayIterations}")
    val catalog = new InMemoryCatalog
    createTables(catalog)
    benchmarkPlanner("", PointSql, catalog, cfg.iterations)
    benchmarkPlanner("_relational", RelationalSql, catalog, cfg.iterations)
    (cfg.dataDir, cfg.nativeLib) match
      case (Some(data), Some(lib)) => benchmarkGateway(cfg.gatewayIterations, data, lib)
      case _ => println("gateway/FFM benchmark skipped; pass --data DIR --native-lib FILE to enable it")

  /** DDL of the benchmark schema. */
  private val Ddl = Vector(
    "CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);",
    "CREATE TABLE transfer (id BIGINT PRIMARY KEY, account_id BIGINT NOT NULL, amount BIGINT NOT NULL);"
  )

  /** Registers the benchmark tables in an in-memory catalog. */
  private def createTables(catalog: Catalog): Unit =
    val parser = new SqlParser
    val binder = new Binder(catalog)
    Ddl.foreach { ddl =>
      binder.bind(parser.parse(ddl)) match
        case create: BoundCreateTable => catalog.createEntity(create.name, create.fields, create.primaryKey)
        case _ => throw new IllegalStateException("expected CREATE TABLE")
    }

  /** Times each planning stage of `sql` (parse, bind, logical plan, optimize, physical plan,
    * plan JSON encoding) and the whole pipeline, after a warm-up. Stage names get `suffix`.
    */
  private def benchmarkPlanner(suffix: String, sql: String, catalog: Catalog, iterations: Int): Unit =
    val parser = new SqlParser
    val binder = new Binder(catalog)
    val optimizer = new RuleOptimizer()
    for _ <- 0 until math.min(1000, iterations) do pipeline(parser, binder, optimizer, sql)

    val parseNs = time(iterations) { parser.parse(sql) }
    val ast = parser.parse(sql)
    val bindNs = time(iterations) { binder.bind(ast) }
    val bound = binder.bind(ast).asInstanceOf[BoundSelect]
    val logicalNs = time(iterations) { LogicalPlanner.plan(bound) }
    val logical = LogicalPlanner.plan(bound)
    val optimizeNs = time(iterations) { optimizer.optimize(logical) }
    val optimized = optimizer.optimize(logical)
    val physicalNs = time(iterations) { PhysicalPlanner.plan(optimized) }
    val physical = PhysicalPlanner.plan(optimized)
    val wireNs = time(iterations) { PlanJsonEncoder.encode(physical.plan) }
    val totalNs = time(iterations) { pipeline(parser, binder, optimizer, sql) }

    report(s"sql_parse$suffix", iterations, parseNs)
    report(s"bind_slots$suffix", iterations, bindNs)
    report(s"logical_plan$suffix", iterations, logicalNs)
    report(s"rule_optimize$suffix", iterations, optimizeNs)
    report(s"physical_plan$suffix", iterations, physicalNs)
    report(s"plan_json_encode$suffix", iterations, wireNs)
    report(s"planner_pipeline$suffix", iterations, totalNs)

  /** Runs the full planning pipeline for one SELECT and returns the native plan JSON. */
  private def pipeline(parser: SqlParser, binder: Binder, optimizer: RuleOptimizer, sql: String): String =
    val bound = binder.bind(parser.parse(sql)).asInstanceOf[BoundSelect]
    val logical = LogicalPlanner.plan(bound)
    val optimized = optimizer.optimize(logical)
    PlanJsonEncoder.encode(PhysicalPlanner.plan(optimized).plan)

  /** Times end-to-end SQL execution through FFM: point lookups, a filtered scan with LIMIT,
    * and the relational query over 1,000 accounts and 10,000 transfers.
    */
  private def benchmarkGateway(iterations: Int, dataDir: Path, nativeLib: Path): Unit =
    Files.createDirectories(dataDir)
    val catalog = new FileCatalog(dataDir.resolve("catalog.properties"))
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    try
      val db = new AdaptiveDatabase(catalog, native)
      if catalog.entity("account").isEmpty then
        Ddl.foreach(db.execute)
        for i <- 1 to 1000 do db.execute(s"INSERT INTO account VALUES ($i, ${i * 10L}, 'owner-${i % 50}');")
        for i <- 1 to 10000 do db.execute(s"INSERT INTO transfer VALUES ($i, ${1 + i % 1000}, ${i % 97});")

      for _ <- 0 until math.min(100, iterations) do db.execute("SELECT * FROM account WHERE id = 500;")
      val pointNs = time(iterations) { db.execute("SELECT * FROM account WHERE id = 500;") }
      val scanIterations = math.max(1, iterations / 10)
      val scanNs = time(scanIterations) { db.execute("SELECT id, balance FROM account WHERE balance >= 9000 LIMIT 100;") }
      val relationalIterations = math.max(1, iterations / 100)
      val relationalNs = time(relationalIterations) { db.execute(RelationalSql) }
      report("gateway_point_lookup_sql_to_rust", iterations, pointNs)
      report("gateway_scan_filter_limit_sql_to_rust", scanIterations, scanNs)
      report("gateway_join_aggregate_top_k_sql_to_rust", relationalIterations, relationalNs)
    finally native.close()

  /** Runs `body` `iterations` times, keeping its result alive so the JIT cannot elide it.
    *
    * @return elapsed wall-clock nanoseconds
    */
  private def time(iterations: Int)(body: => Any): Long =
    val start = System.nanoTime()
    var i = 0
    var sink: Any = null
    while i < iterations do
      sink = body
      i += 1
    if sink == null then ()
    System.nanoTime() - start

  /** Prints throughput and per-operation latency of one benchmark. */
  private def report(name: String, iterations: Int, totalNs: Long): Unit =
    val nsOp = totalNs.toDouble / math.max(1, iterations)
    val ops = if totalNs == 0 then 0.0 else iterations.toDouble / (totalNs.toDouble / 1e9)
    println(f"$name%-42s ops/s=$ops%12.0f ns/op=$nsOp%12.1f")

  /** Parses `--iterations`, `--gateway-iterations`, `--data`, `--native-lib`, `--workloads`,
    * `--out`, `--scale` and `--warmup-ms` into `cfg`.
    */
  @tailrec
  private def parseArgs(args: List[String], cfg: Config): Config = args match
    case Nil => cfg
    case "--iterations" :: value :: tail => parseArgs(tail, cfg.copy(iterations = value.toInt))
    case "--gateway-iterations" :: value :: tail => parseArgs(tail, cfg.copy(gatewayIterations = value.toInt))
    case "--data" :: value :: tail => parseArgs(tail, cfg.copy(dataDir = Some(Path.of(value).toAbsolutePath.normalize())))
    case "--native-lib" :: value :: tail => parseArgs(tail, cfg.copy(nativeLib = Some(Path.of(value).toAbsolutePath.normalize())))
    case "--workloads" :: tail => parseArgs(tail, cfg.copy(workloads = true))
    case "--scale" :: value :: tail => parseArgs(tail, cfg.copy(scale = value.toInt))
    case "--warmup-ms" :: value :: tail => parseArgs(tail, cfg.copy(warmupMs = value.toLong))
    case "--out" :: value :: tail => parseArgs(tail, cfg.copy(out = Some(Path.of(value).toAbsolutePath.normalize())))
    case other :: _ => throw new IllegalArgumentException(s"unknown/incomplete argument: $other")
