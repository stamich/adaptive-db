package io.adb.benchmark

import io.adb.catalog.{FileCatalog, InMemoryCatalog}
import io.adb.ffm.{NativeDatabase, NativeException}
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import io.adb.logical.{Binder, BoundSelect, LogicalPlan, OptimizerMode}
import io.adb.model.*
import io.adb.optimizer.OptimizerConfig
import io.adb.physical.PlanJsonEncoder
import io.adb.sql.SqlParser
import io.adb.statistics.*
import java.nio.charset.StandardCharsets
import java.nio.file.{Files, Path}
import java.util.Optional
import scala.jdk.CollectionConverters.*

/** Milestone 2.2.3 workload benchmark over the native engine.
  *
  * The dataset is loaded by the Rust `workloads --prepare` binary (the only definition of the
  * data; `scripts/benchmark-ffi.sh` runs it); this benchmark creates the matching catalog in the
  * same order, so entity and field ids line up, checks every row count, and then measures:
  *
  *  - '''workloads A and B''' on three paths: `planning_only` (parse, bind, plan, encode),
  *    `ffi_prepared_plan` (a prepared plan through Java FFM: boundary + engine) and
  *    `scala_cbo_ffi_rust` (the whole SQL path); `rust_native` is measured by the Rust binary;
  *  - '''join order''': a chain (`jo_*`) and a star schema (`st_*`) whose SQL order builds the
  *    fact table, in cost and rule mode (rule mode may exceed the engine's limits at large
  *    scale, which is reported, not hidden);
  *  - '''estimation''': skewed (Zipf) values, a perfectly correlated predicate pair, a range,
  *    and stale statistics before and after `ANALYZE`, each with its estimated and actual rows;
  *  - '''planner''': planning time of chain and star queries of 2–12 relations;
  *  - '''calibration''': measured engine time per estimated cost unit for every workload.
  *
  * Every timed path warms up for a minimum time first (JIT), and reports p50, p95 and p99.
  */
object WorkloadBenchmark:
  /** Catalog of the dataset, in the Rust `dataset::TABLES` order (entity ids 1..10). */
  val Ddl: Vector[String] = Vector(
    "CREATE TABLE bench_customer (id BIGINT PRIMARY KEY, name STRING NOT NULL, segment BIGINT NOT NULL)",
    "CREATE TABLE bench_orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL REFERENCES bench_customer(id) NOT ENFORCED, amount BIGINT NOT NULL)",
    "CREATE TABLE jo_c (id BIGINT PRIMARY KEY, tag BIGINT NOT NULL)",
    "CREATE TABLE jo_b (id BIGINT PRIMARY KEY, c_id BIGINT NOT NULL REFERENCES jo_c(id) NOT ENFORCED)",
    "CREATE TABLE jo_a (id BIGINT PRIMARY KEY, b_id BIGINT NOT NULL REFERENCES jo_b(id) NOT ENFORCED, v BIGINT NOT NULL)",
    "CREATE TABLE st_region (id BIGINT PRIMARY KEY, tag BIGINT NOT NULL)",
    "CREATE TABLE st_dim (id BIGINT PRIMARY KEY, region_id BIGINT NOT NULL REFERENCES st_region(id) NOT ENFORCED, name STRING NOT NULL)",
    "CREATE TABLE st_fact (id BIGINT PRIMARY KEY, dim_id BIGINT NOT NULL REFERENCES st_dim(id) NOT ENFORCED, v BIGINT NOT NULL)",
    "CREATE TABLE sk_events (id BIGINT PRIMARY KEY, kind BIGINT NOT NULL, region BIGINT NOT NULL, v BIGINT NOT NULL)",
    "CREATE TABLE sk_stale (id BIGINT PRIMARY KEY, kind BIGINT NOT NULL, v BIGINT NOT NULL)"
  )

  /** Rows per table at scale factor 1, and whether they grow with the scale (as in Rust). */
  val BaseRows: Vector[(String, Long, Boolean)] = Vector(
    ("bench_customer", 100L, true), ("bench_orders", 1_000L, true), ("jo_c", 20L, false), ("jo_b", 2_000L, true),
    ("jo_a", 20_000L, true), ("st_region", 100L, false), ("st_dim", 1_000L, true), ("st_fact", 100_000L, true),
    ("sk_events", 100_000L, true), ("sk_stale", 100_000L, true)
  )

  /** Workload A: hash join + TopK. */
  val WorkloadA: String =
    "SELECT c.name, o.amount FROM bench_customer c JOIN bench_orders o ON c.id = o.customer_id ORDER BY o.amount DESC LIMIT 10"
  /** Workload B: hash join + aggregate + TopK. */
  val WorkloadB: String =
    "SELECT c.name, SUM(o.amount) AS total FROM bench_customer c JOIN bench_orders o ON c.id = o.customer_id " +
      "GROUP BY c.name ORDER BY total DESC LIMIT 10"
  /** Chain join order: SQL order joins the largest table first. */
  val ChainQuery: String =
    "SELECT a.v, c.tag FROM jo_a a JOIN jo_b b ON a.b_id = b.id JOIN jo_c c ON b.c_id = c.id WHERE c.tag = 3"
  /** Star join order: SQL order builds the fact table (rule mode materializes all of it). */
  val StarQuery: String =
    "SELECT COUNT(*) AS n, SUM(f.v) AS total FROM st_dim d JOIN st_fact f ON f.dim_id = d.id " +
      "JOIN st_region r ON d.region_id = r.id WHERE r.tag = 7"
  /** Estimation queries over skewed, correlated and stale data. */
  val EstimationQueries: Vector[(String, String)] = Vector(
    "zipf_most_common" -> "SELECT COUNT(*) FROM sk_events WHERE kind = 0",
    "zipf_rare" -> "SELECT COUNT(*) FROM sk_events WHERE kind = 37",
    "correlated_pair" -> "SELECT COUNT(*) FROM sk_events WHERE kind = 7 AND region = 107",
    "range" -> "SELECT COUNT(*) FROM sk_events WHERE v < 3"
  )

  /** Benchmark settings.
    *
    * @param dataDir    directory with the prepared native database in `rust/`
    * @param nativeLib  native library
    * @param iterations timed iterations per path
    * @param scale      scale factor the data was prepared with
    * @param warmupMs   minimum warm-up time per timed path
    * @param out        JSON report path
    */
  final case class Settings(dataDir: Path, nativeLib: Path, iterations: Int, scale: Int, warmupMs: Long, out: Option[Path])

  /** Runs everything and writes the JSON report. */
  def run(settings: Settings): Unit =
    val catalogPath = settings.dataDir.resolve("catalog.properties")
    require(Files.isDirectory(settings.dataDir.resolve("rust")), s"${settings.dataDir}/rust not found; prepare it with `workloads --prepare`")
    require(!Files.exists(catalogPath), s"$catalogPath exists; use a freshly prepared directory")
    val catalog = new FileCatalog(catalogPath)
    val native = NativeDatabase.open(settings.nativeLib, settings.dataDir.resolve("rust"))
    try
      val db = new AdaptiveDatabase(catalog, native)
      Ddl.foreach(db.execute)
      val counts = verifyRowCounts(db, settings.scale)
      BaseRows.map(_._1).filterNot(_ == "sk_stale").foreach(table => db.execute(s"ANALYZE $table"))
      val timer = Timer(settings.iterations, settings.warmupMs)

      val workloads = Vector("A_hash_join_topk" -> WorkloadA, "B_hash_join_aggregate_topk" -> WorkloadB).map { (name, sql) =>
        val prepared = preparedPlan(db, catalog, sql)
        name -> obj(
          "planning_only" -> timer.measure(planOnly(db, catalog, sql)).json,
          "ffi_prepared_plan" -> timer.measure(drain(native, prepared)).json,
          "scala_cbo_ffi_rust" -> timer.measure(checksum(db.execute(sql))).json,
          "estimated_root_cost" -> num(rootCost(db, catalog, sql)),
          "max_q_error" -> num(maxQError(db, sql))
        )
      }
      val joinOrder = Vector("chain" -> ChainQuery, "star" -> StarQuery).map((name, sql) => name -> modes(db, catalog, native, sql, timer))
      val report = obj(
        "milestone" -> str("2.2.3"),
        "host" -> obj(
          "os" -> str(System.getProperty("os.name")), "arch" -> str(System.getProperty("os.arch")),
          "cpu" -> str(cpuModel()), "cpus" -> num(Runtime.getRuntime.availableProcessors()),
          "java" -> str(System.getProperty("java.version")), "max_heap_mib" -> num(Runtime.getRuntime.maxMemory() / 1048576)
        ),
        "setup" -> obj("scale" -> num(settings.scale), "iterations" -> num(settings.iterations), "warmup_ms" -> num(settings.warmupMs),
          "rows" -> obj(counts.map((t, n) => t -> num(n))*)),
        "workloads" -> obj(workloads*),
        "join_order" -> obj(joinOrder*),
        "estimation" -> estimationSection(db),
        "planner" -> plannerSection(timer),
        "calibration" -> calibration(workloads.map((n, s) => n -> s) ++ joinOrder.map((n, s) => s"join_order_$n" -> s))
      )
      println(report)
      settings.out.foreach(path => Files.writeString(path, report + "\n", StandardCharsets.UTF_8))
    finally native.close()

  /** Checks that every table holds the rows the scale factor implies; returns the counts. */
  private def verifyRowCounts(db: AdaptiveDatabase, scale: Int): Vector[(String, Long)] =
    BaseRows.map { (table, base, scaled) =>
      val expected = if scaled then base * scale else base
      val actual = db.execute(s"SELECT COUNT(*) FROM $table").rows.head.head.asInstanceOf[java.lang.Long].longValue
      require(actual == expected, s"$table holds $actual rows, expected $expected at scale $scale")
      table -> actual
    }

  /** One query in cost and in rule mode: timing (or the error that stopped it), plan cost,
    * worst q-error and join tree per mode.
    */
  private def modes(db: AdaptiveDatabase, catalog: FileCatalog, native: NativeDatabase, sql: String, timer: Timer): String =
    val results = Vector("cost", "rule").map { mode =>
      db.execute(s"SET optimizer = $mode")
      val tree = joinTree(db.plan(bind(catalog, sql)).optimized)
      val section =
        try
          val prepared = preparedPlan(db, catalog, sql)
          obj(
            "ffi_prepared_plan" -> timer.measure(drain(native, prepared)).json,
            "estimated_root_cost" -> num(rootCost(db, catalog, sql)),
            "max_q_error" -> num(maxQError(db, sql)),
            "join_tree" -> str(tree)
          )
        catch
          case error: NativeException =>
            obj("error" -> str(s"${error.status()}: ${error.getMessage}"), "join_tree" -> str(tree))
      mode -> section
    }
    db.execute("SET optimizer = cost")
    obj(results*)

  /** Estimated versus actual rows of every estimation query, plus stale statistics before and
    * after `ANALYZE sk_stale`.
    */
  private def estimationSection(db: AdaptiveDatabase): String =
    val queries = EstimationQueries.map((name, sql) => name -> obj(filterEstimate(db, sql)*))
    val staleSql = "SELECT COUNT(*) FROM sk_stale WHERE kind = 0"
    val staleWarning = db.execute("EXPLAIN " + staleSql).rows.map(_.head.toString).exists(_.contains("statistics of sk_stale are stale"))
    val stale = filterEstimate(db, staleSql) :+ ("stale_warning" -> staleWarning.toString)
    db.execute("ANALYZE sk_stale")
    val refreshed = filterEstimate(db, staleSql)
    obj((queries ++ Vector("stale_statistics" -> obj(stale*), "after_analyze" -> obj(refreshed*)))*)

  /** `estimated`, `actual` and `q_error` of the filter of `sql`, read from EXPLAIN ANALYZE. */
  private def filterEstimate(db: AdaptiveDatabase, sql: String): Vector[(String, String)] =
    val text = db.execute("EXPLAIN ANALYZE " + sql).rows.map(_.head.toString).mkString("\n")
    raw"filter rows=(\d+) est=(\d+) q=([0-9.]+)".r.findFirstMatchIn(text) match
      case Some(m) => Vector("estimated" -> num(m.group(2).toLong), "actual" -> num(m.group(1).toLong), "q_error" -> num(m.group(3).toDouble))
      case None => Vector("error" -> str("no filter in the profile"))

  /** Planning time (parse, bind, rules, join order, strategies) of chain and star queries over
    * 2..12 relations, with statistics but without the engine.
    */
  private def plannerSection(timer: Timer): String =
    val catalog = new InMemoryCatalog
    val sizes = Vector(1_000_000L, 20_000L, 500L, 80_000L, 3_000L, 250_000L, 40L, 9_000L, 120_000L, 700L, 60_000L, 5_000L)
    for i <- sizes.indices do
      catalog.createEntity(s"p$i", Vector(("id", DataType.Int64, false), ("k", DataType.Int64, false), ("v", DataType.Int64, false)), "id")
    val statistics = StatisticsProvider.of(sizes.indices.map { i =>
      EntityStatistics(TableStatistics(catalog.entity(s"p$i").get.id, sizes(i), 24, 1, 0, sizes(i), true, Map.empty), 0)
    }*)
    val binder = new Binder(catalog)
    val parser = new SqlParser
    /** `SELECT ... FROM p0 t0 JOIN p1 t1 ON ...` with each relation joined to its predecessor (chain) or to t0 (star). */
    def query(n: Int, star: Boolean): String =
      val joins = (1 until n).map(i => s" JOIN p$i t$i ON t${if star then 0 else i - 1}.k = t$i.id").mkString
      s"SELECT t0.v FROM p0 t0$joins WHERE t${n - 1}.v < 10"
    val rows = for shape <- Vector("chain", "star"); n <- Vector(2, 4, 6, 8, 10, 11, 12) yield
      val sql = query(n, shape == "star")
      val plan = () => AdaptiveDatabase.plan(binder.bind(parser.parse(sql)).asInstanceOf[BoundSelect], catalog, statistics, OptimizerConfig.Default, OptimizerMode.Cost)
      val method = plan().physical.decisions.find(_.operator == "join order").fold("")(_.reason.takeWhile(_ != ';'))
      val timing = timer.measure { plan(); 0L }
      s"${shape}_$n" -> obj("relations" -> num(n), "p50_us" -> num(timing.percentile(0.5) * 1000), "p99_us" -> num(timing.percentile(0.99) * 1000), "method" -> str(method))
    obj(rows*)

  /** Engine milliseconds per 1,000 estimated cost units (prepared-plan path, so planning is
    * excluded) for every measured plan, and the spread between them.
    */
  private def calibration(sections: Vector[(String, String)]): String =
    val cost = raw""""estimated_root_cost":([0-9.Ee+-]+)""".r
    val p50 = raw""""ffi_prepared_plan":\{[^}]*"p50_ms":([0-9.Ee+-]+)""".r
    val points = sections.flatMap { (name, section) =>
      // A section may hold one plan (workloads) or a "cost" and a "rule" plan (join order).
      val parts = if section.contains("\"cost\":{") then
        Vector(s"${name}_cost" -> section.substring(0, section.indexOf("\"rule\":{")), s"${name}_rule" -> section.substring(section.indexOf("\"rule\":{")))
      else Vector(name -> section)
      parts.flatMap { (label, text) =>
        for c <- cost.findFirstMatchIn(text).map(_.group(1).toDouble); m <- p50.findFirstMatchIn(text).map(_.group(1).toDouble) if c > 0
        yield label -> m * 1000 / c
      }
    }
    val ratios = points.map(_._2)
    obj(
      "ms_per_1k_cost" -> obj(points.map((label, r) => label -> num(r))*),
      "spread" -> num(if ratios.isEmpty then Double.NaN else ratios.max / ratios.min)
    )

  /** `((a JOIN b) JOIN c)` shape of the joins of a logical plan. */
  private def joinTree(plan: LogicalPlan): String = plan match
    case LogicalPlan.Join(left, right, _, _) => s"(${joinTree(left)} JOIN ${joinTree(right)})"
    case LogicalPlan.TableScan(relation) => relation.alias
    case LogicalPlan.PointLookup(relation, _) => relation.alias
    case other => other.children.map(joinTree).mkString(",")

  /** Parses, binds, plans and encodes `sql` without executing it; returns the plan size. */
  private def planOnly(db: AdaptiveDatabase, catalog: FileCatalog, sql: String): Long =
    preparedPlan(db, catalog, sql).length.toLong

  /** Physical plan JSON of `sql` in the session's current mode. */
  private def preparedPlan(db: AdaptiveDatabase, catalog: FileCatalog, sql: String): String =
    PlanJsonEncoder.encode(db.plan(bind(catalog, sql)).physical.plan)

  /** Estimated weighted cost of the plan's root. */
  private def rootCost(db: AdaptiveDatabase, catalog: FileCatalog, sql: String): Double =
    db.plan(bind(catalog, sql)).physical.estimates.get(0).fold(Double.NaN)(_.cost)

  /** Worst q-error reported by EXPLAIN ANALYZE. */
  private def maxQError(db: AdaptiveDatabase, sql: String): Double =
    val text = db.execute("EXPLAIN ANALYZE " + sql).rows.map(_.head.toString).mkString("\n")
    raw"max q-error ([0-9.]+)".r.findFirstMatchIn(text).fold(Double.NaN)(_.group(1).toDouble)

  /** Binds a SELECT. */
  private def bind(catalog: FileCatalog, sql: String): BoundSelect =
    new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]

  /** Executes a prepared plan and sums its INT64 values (the Rust binary's checksum). */
  private def drain(native: NativeDatabase, json: String): Long =
    val query = native.execute(json, Optional.empty[java.lang.Long]())
    try
      var sum = 0L
      var next = query.nextBatch()
      while next.isPresent do
        for column <- next.get().columns().asScala; value <- column.values().asScala do
          value match
            case v: java.lang.Long => sum += v
            case _ => ()
        next = query.nextBatch()
      sum
    finally query.close()

  /** Sum of the INT64 values of a result. */
  private def checksum(result: QueryResult): Long =
    result.rows.iterator.flatten.collect { case v: java.lang.Long => v.longValue }.sum

  /** CPU model from `/proc/cpuinfo` (Linux), or "unknown". */
  private def cpuModel(): String =
    try
      Files.readAllLines(Path.of("/proc/cpuinfo")).asScala.find(_.startsWith("model name"))
        .map(_.split(":", 2)(1).trim).getOrElse("unknown")
    catch case _: java.io.IOException => "unknown"

  /** Timed runs with a time-based warm-up.
    *
    * @param iterations timed iterations
    * @param warmupMs   minimum warm-up time (at least 10 runs)
    */
  private final case class Timer(iterations: Int, warmupMs: Long):
    /** Warms `body` up, then times `iterations` runs. */
    def measure(body: => Long): Timing =
      val warmStart = System.nanoTime()
      var warm = 0
      while warm < 10 || (System.nanoTime() - warmStart) / 1_000_000 < warmupMs do
        body
        warm += 1
      var last = 0L
      val samples = Vector.fill(iterations) {
        val start = System.nanoTime()
        last = body
        (System.nanoTime() - start) / 1e6
      }
      Timing(samples.sorted, last, warm)

  /** Timings of one path.
    *
    * @param samples    milliseconds per iteration, sorted
    * @param checksum   checksum of the last iteration
    * @param warmupRuns runs before timing started
    */
  private final case class Timing(samples: Vector[Double], checksum: Long, warmupRuns: Int):
    /** `{"iterations":…,"warmup_runs":…,"p50_ms":…,"p95_ms":…,"p99_ms":…,"mean_ms":…,"checksum":…}`. */
    def json: String = obj(
      "iterations" -> num(samples.size), "warmup_runs" -> num(warmupRuns), "p50_ms" -> num(percentile(0.5)),
      "p95_ms" -> num(percentile(0.95)), "p99_ms" -> num(percentile(0.99)), "mean_ms" -> num(samples.sum / samples.size),
      "checksum" -> num(checksum)
    )
    /** The `p`-th percentile. */
    def percentile(p: Double): Double = samples(((samples.size - 1) * p).round.toInt)

  /** A JSON object of already-encoded values. */
  private def obj(fields: (String, String)*): String = fields.map((k, v) => s"${PlanJsonEncoder.quote(k)}:$v").mkString("{", ",", "}")
  /** A JSON string. */
  private def str(value: String): String = PlanJsonEncoder.quote(value)
  /** A JSON integer. */
  private def num(value: Long): String = value.toString
  /** A JSON number (`null` when not finite). */
  private def num(value: Double): String =
    if value.isNaN || value.isInfinite then "null"
    else if value == math.rint(value) && math.abs(value) < 1e15 then value.toLong.toString
    else value.toString
