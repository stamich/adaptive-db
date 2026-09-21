package io.adb.benchmark

import io.adb.catalog.{FileCatalog, InMemoryCatalog}
import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import io.adb.logical.{Binder, BoundSelect, LogicalPlanner}
import io.adb.optimizer.RuleOptimizer
import io.adb.physical.{PhysicalPlanner, PlanJsonEncoder}
import io.adb.sql.SqlParser

import java.nio.file.{Files, Path}
import scala.annotation.tailrec
import scala.util.Using

/** Milestone 2.0.2 benchmark separating JVM planning cost from native end-to-end execution. */
object BenchmarkMain:
  private final case class Config(iterations: Int = 10000, gatewayIterations: Int = 1000, dataDir: Option[Path] = None, nativeLib: Option[Path] = None)

  def main(args: Array[String]): Unit =
    val cfg = parseArgs(args.toList, Config())
    println(s"Adaptive DB 2.0.2 JVM benchmark iterations=${cfg.iterations} gatewayIterations=${cfg.gatewayIterations}")
    benchmarkPlanner(cfg.iterations)
    (cfg.dataDir, cfg.nativeLib) match
      case (Some(data), Some(lib)) => benchmarkGateway(cfg.gatewayIterations, data, lib)
      case _ => println("gateway/FFM benchmark skipped; pass --data DIR --native-lib FILE to enable it")

  private def benchmarkPlanner(iterations: Int): Unit =
    val catalog = new InMemoryCatalog
    val parser = new SqlParser
    val binder = new Binder(catalog)
    val optimizer = new RuleOptimizer()
    binder.bind(parser.parse("CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);")) match
      case create: io.adb.logical.BoundCreateTable => catalog.createEntity(create.name, create.fields, create.primaryKey)
      case _ => throw new IllegalStateException("expected CREATE TABLE")

    val sql = "SELECT id, owner, balance FROM account WHERE id = 42 LIMIT 10;"
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
    val wireNs = time(iterations) { PlanJsonEncoder.encode(physical) }
    val totalNs = time(iterations) { pipeline(parser, binder, optimizer, sql) }

    report("sql_parse", iterations, parseNs)
    report("bind", iterations, bindNs)
    report("logical_plan", iterations, logicalNs)
    report("optimize", iterations, optimizeNs)
    report("physical_plan", iterations, physicalNs)
    report("plan_json_encode", iterations, wireNs)
    report("planner_pipeline", iterations, totalNs)

  private def pipeline(parser: SqlParser, binder: Binder, optimizer: RuleOptimizer, sql: String): String =
    val bound = binder.bind(parser.parse(sql)).asInstanceOf[BoundSelect]
    val logical = LogicalPlanner.plan(bound)
    val optimized = optimizer.optimize(logical)
    PlanJsonEncoder.encode(PhysicalPlanner.plan(optimized))

  private def benchmarkGateway(iterations: Int, dataDir: Path, nativeLib: Path): Unit =
    Files.createDirectories(dataDir)
    val catalog = new FileCatalog(dataDir.resolve("catalog.properties"))
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    try
      val db = new AdaptiveDatabase(catalog, native)
      if catalog.entity("bench").isEmpty then
        db.execute("CREATE TABLE bench (id BIGINT PRIMARY KEY, value BIGINT NOT NULL, label STRING);")
        for i <- 1 to 1000 do db.execute(s"INSERT INTO bench VALUES ($i, ${i * 10L}, 'row-$i');")

      for _ <- 0 until math.min(100, iterations) do db.execute("SELECT * FROM bench WHERE id = 500;")
      val pointNs = time(iterations) { db.execute("SELECT * FROM bench WHERE id = 500;") }
      val scanNs = time(math.max(1, iterations / 10)) { db.execute("SELECT id, value FROM bench WHERE value >= 9000 LIMIT 100;") }
      report("gateway_point_lookup_sql_to_rust", iterations, pointNs)
      report("gateway_scan_filter_limit_sql_to_rust", math.max(1, iterations / 10), scanNs)
    finally native.close()

  private def time(iterations: Int)(body: => Any): Long =
    val start = System.nanoTime()
    var i = 0
    var sink: Any = null
    while i < iterations do
      sink = body
      i += 1
    if sink == null then ()
    System.nanoTime() - start

  private def report(name: String, iterations: Int, totalNs: Long): Unit =
    val nsOp = totalNs.toDouble / math.max(1, iterations)
    val ops = if totalNs == 0 then 0.0 else iterations.toDouble / (totalNs.toDouble / 1e9)
    println(f"$name%-38s ops/s=$ops%12.0f ns/op=$nsOp%12.1f")

  @tailrec
  private def parseArgs(args: List[String], cfg: Config): Config = args match
    case Nil => cfg
    case "--iterations" :: value :: tail => parseArgs(tail, cfg.copy(iterations = value.toInt))
    case "--gateway-iterations" :: value :: tail => parseArgs(tail, cfg.copy(gatewayIterations = value.toInt))
    case "--data" :: value :: tail => parseArgs(tail, cfg.copy(dataDir = Some(Path.of(value).toAbsolutePath.normalize())))
    case "--native-lib" :: value :: tail => parseArgs(tail, cfg.copy(nativeLib = Some(Path.of(value).toAbsolutePath.normalize())))
    case other :: _ => throw new IllegalArgumentException(s"unknown/incomplete argument: $other")
