package io.adb.benchmark

import io.adb.catalog.FileCatalog
import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import io.adb.logical.{Binder, BoundSelect, LogicalPlan}
import io.adb.physical.PlanJsonEncoder
import io.adb.sql.SqlParser
import java.nio.charset.StandardCharsets
import java.nio.file.{Files, Path}
import java.util.Optional
import scala.jdk.CollectionConverters.*

/** Milestone 2.2.3 workload benchmark over the native engine.
  *
  * The same tables as the Rust `workloads` binary (identical formulas), the same workloads,
  * three paths:
  *
  *  - `ffi_prepared_plan`: the cost-based plan is built once; each iteration sends its JSON
  *    through Java FFM and drains the batches (boundary + engine);
  *  - `scala_cbo_ffi_rust`: each iteration is the full SQL path (parse, bind, rule rewrites,
  *    join ordering, cost-based strategies, FFM, engine, materialization);
  *  - `rust_native` is measured by the Rust binary.
  *
  * A join-order workload (`jo_a` 20,000 rows → `jo_b` 2,000 → `jo_c` 20, a selective filter on
  * `jo_c`) runs in cost and rule mode, and every workload reports its root cost estimate next
  * to the measured time (`calibration`) and the worst q-error of EXPLAIN ANALYZE.
  */
object WorkloadBenchmark:
  /** Workload A: hash join + TopK. */
  val WorkloadA: String =
    "SELECT c.name, o.amount FROM bench_customer c JOIN bench_orders o ON c.id = o.customer_id ORDER BY o.amount DESC LIMIT 10"
  /** Workload B: hash join + aggregate + TopK. */
  val WorkloadB: String =
    "SELECT c.name, SUM(o.amount) AS total FROM bench_customer c JOIN bench_orders o ON c.id = o.customer_id " +
      "GROUP BY c.name ORDER BY total DESC LIMIT 10"
  /** Join-order workload: SQL order joins the 20,000-row table first. */
  val JoinOrder: String =
    "SELECT a.v, c.tag FROM jo_a a JOIN jo_b b ON a.b_id = b.id JOIN jo_c c ON b.c_id = c.id WHERE c.tag = 3"

  /** Runs everything against a fresh database in `dataDir` and writes the JSON report to `out`. */
  def run(dataDir: Path, nativeLib: Path, iterations: Int, out: Option[Path]): Unit =
    require(!Files.exists(dataDir) || Files.list(dataDir).findAny().isEmpty, s"$dataDir must be empty or absent")
    Files.createDirectories(dataDir)
    val catalog = new FileCatalog(dataDir.resolve("catalog.properties"))
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    try
      val db = new AdaptiveDatabase(catalog, native)
      load(db)
      val workloads = Vector("A_hash_join_topk" -> WorkloadA, "B_hash_join_aggregate_topk" -> WorkloadB).map { (name, sql) =>
        val prepared = preparedPlan(db, catalog, sql)
        val ffi = measure(iterations)(drain(native, prepared))
        val full = measure(iterations)(checksum(db.execute(sql)))
        name -> obj(
          "ffi_prepared_plan" -> ffi.json,
          "scala_cbo_ffi_rust" -> full.json,
          "estimated_root_cost" -> num(rootCost(db, catalog, sql)),
          "max_q_error" -> num(maxQError(db, sql))
        )
      }
      val joinOrder = joinOrderSection(db, catalog, math.max(5, iterations / 5))
      val report = obj(
        "milestone" -> str("2.2.3"),
        "host" -> obj(
          "os" -> str(System.getProperty("os.name")), "arch" -> str(System.getProperty("os.arch")),
          "cpus" -> num(Runtime.getRuntime.availableProcessors()), "java" -> str(System.getProperty("java.version"))
        ),
        "setup" -> obj("bench_customer" -> num(100), "bench_orders" -> num(1000), "iterations" -> num(iterations)),
        "workloads" -> obj(workloads*),
        "join_order" -> joinOrder,
        "calibration" -> obj(workloads.map((name, section) => name -> calibration(section))*)
      )
      println(report)
      out.foreach(path => Files.writeString(path, report + "\n", StandardCharsets.UTF_8))
    finally native.close()

  /** Creates and loads the benchmark tables (same formulas as the Rust binary), then ANALYZE. */
  private def load(db: AdaptiveDatabase): Unit =
    db.execute("CREATE TABLE bench_customer (id BIGINT PRIMARY KEY, name STRING NOT NULL, segment BIGINT NOT NULL)")
    db.execute("CREATE TABLE bench_orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL REFERENCES bench_customer(id) NOT ENFORCED, amount BIGINT NOT NULL)")
    for id <- 1 to 100 do db.execute(s"INSERT INTO bench_customer VALUES ($id, 'customer-$id', ${id % 5})")
    for id <- 1 to 1000 do db.execute(s"INSERT INTO bench_orders VALUES ($id, ${1 + (id * 7) % 100}, ${(id * 37) % 1000 + 1})")
    db.execute("CREATE TABLE jo_c (id BIGINT PRIMARY KEY, tag BIGINT NOT NULL)")
    db.execute("CREATE TABLE jo_b (id BIGINT PRIMARY KEY, c_id BIGINT NOT NULL REFERENCES jo_c(id) NOT ENFORCED)")
    db.execute("CREATE TABLE jo_a (id BIGINT PRIMARY KEY, b_id BIGINT NOT NULL REFERENCES jo_b(id) NOT ENFORCED, v BIGINT NOT NULL)")
    for id <- 1 to 20 do db.execute(s"INSERT INTO jo_c VALUES ($id, ${id % 10})")
    for id <- 1 to 2000 do db.execute(s"INSERT INTO jo_b VALUES ($id, ${1 + (id * 7) % 20})")
    for id <- 1 to 20000 do db.execute(s"INSERT INTO jo_a VALUES ($id, ${1 + (id * 13) % 2000}, ${id % 1000})")
    db.execute("ANALYZE")

  /** Cost mode versus rule mode on the join-order workload. */
  private def joinOrderSection(db: AdaptiveDatabase, catalog: FileCatalog, iterations: Int): String =
    val modes = Vector("cost", "rule").map { mode =>
      db.execute(s"SET optimizer = $mode")
      val timing = measure(iterations)(checksum(db.execute(JoinOrder)))
      mode -> obj(
        "timing" -> timing.json,
        "estimated_root_cost" -> num(rootCost(db, catalog, JoinOrder)),
        "max_q_error" -> num(maxQError(db, JoinOrder)),
        "join_tree" -> str(joinTree(db.plan(bind(catalog, JoinOrder)).optimized))
      )
    }
    db.execute("SET optimizer = cost")
    obj(("rows" -> obj("jo_a" -> num(20000), "jo_b" -> num(2000), "jo_c" -> num(20))) +: modes*)

  /** `((a JOIN b) JOIN c)` shape of the joins of a logical plan. */
  private def joinTree(plan: LogicalPlan): String = plan match
    case LogicalPlan.Join(left, right, _, _) => s"(${joinTree(left)} JOIN ${joinTree(right)})"
    case LogicalPlan.TableScan(relation) => relation.alias
    case LogicalPlan.PointLookup(relation, _) => relation.alias
    case other => other.children.map(joinTree).mkString(",")

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

  /** Timings of one path.
    *
    * @param samples  milliseconds per iteration, sorted
    * @param checksum checksum of the last iteration
    */
  private final case class Timing(samples: Vector[Double], checksum: Long):
    /** `{"iterations":…,"p50_ms":…,"p95_ms":…,"mean_ms":…,"checksum":…}`. */
    def json: String = obj(
      "iterations" -> num(samples.size), "p50_ms" -> num(percentile(0.5)), "p95_ms" -> num(percentile(0.95)),
      "mean_ms" -> num(samples.sum / samples.size), "checksum" -> num(checksum)
    )
    /** The `p`-th percentile. */
    def percentile(p: Double): Double = samples(((samples.size - 1) * p).round.toInt)

  /** Runs `body` 10 times to warm up, then `iterations` timed times. */
  private def measure(iterations: Int)(body: => Long): Timing =
    for _ <- 0 until 10 do body
    var last = 0L
    val samples = Vector.fill(iterations) {
      val start = System.nanoTime()
      last = body
      (System.nanoTime() - start) / 1e6
    }
    Timing(samples.sorted, last)

  /** `{"ms_per_1k_cost": …}` from a workload section's estimated cost and full-path p50. */
  private def calibration(section: String): String =
    val cost = raw""""estimated_root_cost":([0-9.Ee+-]+)""".r.findFirstMatchIn(section).map(_.group(1).toDouble)
    val ms = raw""""scala_cbo_ffi_rust":\{[^}]*"p50_ms":([0-9.Ee+-]+)""".r.findFirstMatchIn(section).map(_.group(1).toDouble)
    (cost, ms) match
      case (Some(c), Some(m)) if c > 0 => obj("estimated_cost" -> num(c), "p50_ms" -> num(m), "ms_per_1k_cost" -> num(m * 1000 / c))
      case _ => obj()

  /** A JSON object of already-encoded values. */
  private def obj(fields: (String, String)*): String = fields.map((k, v) => s"${PlanJsonEncoder.quote(k)}:$v").mkString("{", ",", "}")
  /** A JSON string. */
  private def str(value: String): String = PlanJsonEncoder.quote(value)
  /** A JSON integer. */
  private def num(value: Long): String = value.toString
  /** A JSON number (`null` when not finite). */
  private def num(value: Double): String = if value.isNaN || value.isInfinite then "null" else if value == math.rint(value) && math.abs(value) < 1e15 then value.toLong.toString else value.toString
