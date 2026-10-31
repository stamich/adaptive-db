package io.adb.benchmark

import io.adb.catalog.{FileCatalog, InMemoryCatalog}
import io.adb.ffm.{NativeDatabase, NativeException}
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import io.adb.logical.{Binder, BoundSelect, LogicalPlan, OptimizerMode}
import io.adb.model.*
import io.adb.optimizer.OptimizerConfig
import io.adb.optimizer.cost.CostWeights
import io.adb.physical.{NodeEstimate, PlanJsonEncoder}
import io.adb.sql.SqlParser
import io.adb.statistics.*
import java.nio.charset.StandardCharsets
import java.nio.file.{Files, Path}
import java.util.Optional
import scala.jdk.CollectionConverters.*

/** Milestone 2.3.0 workload benchmark over the native engine.
  *
  * The dataset is loaded by the Rust `workloads --prepare` binary (the only definition of the
  * data; `scripts/benchmark-ffi.sh` runs it); this benchmark creates the matching catalog in the
  * same order, so entity and field ids line up, checks every row count, and then measures:
  *
  *  - '''workloads A–E''' on three paths: `planning_only` (parse, bind, plan, encode),
  *    `ffi_prepared_plan` (a prepared plan through Java FFM: boundary + engine) and
  *    `scala_cbo_ffi_rust` (the whole SQL path); `rust_native` is measured by the Rust binary;
  *  - '''join order''': a chain (`jo_*`) and a star schema (`st_*`) whose SQL order builds the
  *    fact table, in cost and rule mode (rule mode may exceed the engine's limits at large
  *    scale, which is reported, not hidden);
  *  - '''estimation''': skewed (Zipf) values, a perfectly correlated predicate pair, a range,
  *    and stale statistics before and after `ANALYZE`, each with its estimated and actual rows;
  *  - '''index build''': `CREATE INDEX` over the loaded `ix_account` rows;
  *  - '''selectivity sweep''': a non-covering balance range from 0.01% to 50% of the rows,
  *    run as an index plan (forced), a sequential plan (rule mode) and the plan cost mode
  *    chooses, which shows where the two paths cross and whether the optimizer finds it;
  *  - '''planner''': planning time of chain and star queries of 2–12 relations;
  *  - '''calibration''': for every measured plan, the unweighted cost components of its root,
  *    the weighted cost and the engine time (prepared-plan path), from which
  *    `scripts/fit-cost-weights.py` fits the [[io.adb.optimizer.cost.CostWeights]]; plus the
  *    engine time per cost unit and its spread across plans under the current weights.
  *
  * Every timed path warms up for a minimum time first (JIT), and reports p50, p95, p99, mean,
  * min, max, the standard deviation and the medians of seven consecutive rounds of samples.
  */
object WorkloadBenchmark:
  /** Catalog of the dataset, in the Rust `dataset::TABLES` order (entity ids 1..11). */
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
    "CREATE TABLE sk_stale (id BIGINT PRIMARY KEY, kind BIGINT NOT NULL, v BIGINT NOT NULL)",
    "CREATE TABLE ix_account (id BIGINT PRIMARY KEY, region BIGINT NOT NULL, balance BIGINT NOT NULL, owner STRING NOT NULL, note STRING NOT NULL)"
  )

  /** Indexes of the 2.3.0 workloads, as the Rust `dataset::INDEXES` declares them. */
  val IndexDdl: Vector[(String, String)] = Vector(
    "idx_ix_owner" -> "CREATE INDEX idx_ix_owner ON ix_account (owner) INCLUDE (balance)",
    "idx_ix_balance" -> "CREATE INDEX idx_ix_balance ON ix_account (balance)"
  )

  /** Modulus of the `ix_account.balance` permutation: balances spread over `0..BalanceModulus`. */
  val BalanceModulus: Long = 1_000_000_007L

  /** Rows per table at scale factor 1, and whether they grow with the scale (as in Rust). */
  val BaseRows: Vector[(String, Long, Boolean)] = Vector(
    ("bench_customer", 100L, true), ("bench_orders", 1_000L, true), ("jo_c", 20L, false), ("jo_b", 2_000L, true),
    ("jo_a", 20_000L, true), ("st_region", 100L, false), ("st_dim", 1_000L, true), ("st_fact", 100_000L, true),
    ("sk_events", 100_000L, true), ("sk_stale", 100_000L, true), ("ix_account", 100_000L, true)
  )

  /** Workload A: hash join + TopK. */
  val WorkloadA: String =
    "SELECT c.name, o.amount FROM bench_customer c JOIN bench_orders o ON c.id = o.customer_id ORDER BY o.amount DESC LIMIT 10"
  /** Workload B: hash join + aggregate + TopK. */
  val WorkloadB: String =
    "SELECT c.name, SUM(o.amount) AS total FROM bench_customer c JOIN bench_orders o ON c.id = o.customer_id " +
      "GROUP BY c.name ORDER BY total DESC LIMIT 10"
  /** Workload C: covering equality scan of `idx_ix_owner` (1 / 10,000 of the rows). */
  val WorkloadC: String = "SELECT owner, balance FROM ix_account WHERE owner = 'owner-42'"
  /** Workload D: ordered range of `idx_ix_balance` with row fetch; LIMIT replaces the sort.
    * Balances are unique, so every plan returns the same rows.
    */
  val WorkloadD: String = "SELECT id, balance FROM ix_account WHERE balance >= 10000000 ORDER BY balance LIMIT 100"
  /** Workload E (control): no index on `region`, so a sequential scan with a filter. */
  val WorkloadE: String = "SELECT COUNT(*) FROM ix_account WHERE region = 7"
  /** Fractions of `ix_account` the selectivity sweep selects by a balance range. */
  val SweepFractions: Vector[Double] = Vector(0.0001, 0.001, 0.01, 0.05, 0.2, 0.5)

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
      val indexBuild = IndexDdl.map { (name, ddl) =>
        val start = System.nanoTime()
        db.execute(ddl)
        name -> obj("sql" -> str(ddl), "ms" -> num((System.nanoTime() - start) / 1e6))
      }
      db.execute("ANALYZE ix_account") // index statistics for the cost model
      val timer = Timer(settings.iterations, settings.warmupMs)
      val points = Vector.newBuilder[CalibrationPoint]

      val workloads = Vector(
        "A_hash_join_topk" -> WorkloadA,
        "B_hash_join_aggregate_topk" -> WorkloadB,
        "C_covering_index_eq" -> WorkloadC,
        "D_index_range_order_limit" -> WorkloadD,
        "E_seq_scan_region_eq" -> WorkloadE
      ).map { (name, sql) =>
        val prepared = preparedPlan(db, catalog, sql)
        val engine = timer.measure(drain(native, prepared))
        points += CalibrationPoint(name, rootEstimate(db, catalog, sql), engine.percentile(0.5))
        name -> obj(
          "planning_only" -> timer.measure(planOnly(db, catalog, sql)).json,
          "ffi_prepared_plan" -> engine.json,
          "scala_cbo_ffi_rust" -> timer.measure(checksum(db.execute(sql))).json,
          "access_path" -> str(accessPath(db, catalog, sql)),
          "estimated_root_cost" -> num(rootCost(db, catalog, sql)),
          "max_q_error" -> num(maxQError(db, sql))
        )
      }
      val sweep = selectivitySweep(db, catalog, native, settings.scale, timer, points)
      val joinOrder = Vector("chain" -> ChainQuery, "star" -> StarQuery).map((name, sql) => name -> modes(db, catalog, native, sql, timer, name, points))
      // Scan-only plans (filter + aggregate, nothing hashed) separate the row-operation weight
      // from the hash-row weight in the calibration fit.
      EstimationQueries.foreach { (name, sql) =>
        val engine = timer.measure(drain(native, preparedPlan(db, catalog, sql)))
        points += CalibrationPoint(s"scan_$name", rootEstimate(db, catalog, sql), engine.percentile(0.5))
      }
      val report = obj(
        "milestone" -> str("2.3.0"),
        "host" -> obj(
          "os" -> str(System.getProperty("os.name")), "arch" -> str(System.getProperty("os.arch")),
          "cpu" -> str(cpuModel()), "cpus" -> num(Runtime.getRuntime.availableProcessors()),
          "java" -> str(System.getProperty("java.version")), "max_heap_mib" -> num(Runtime.getRuntime.maxMemory() / 1048576)
        ),
        "setup" -> obj("scale" -> num(settings.scale), "iterations" -> num(settings.iterations), "warmup_ms" -> num(settings.warmupMs),
          "rows" -> obj(counts.map((t, n) => t -> num(n))*)),
        "index_build" -> obj(indexBuild*),
        "workloads" -> obj(workloads*),
        "selectivity_sweep" -> sweep,
        "join_order" -> obj(joinOrder*),
        "estimation" -> estimationSection(db),
        "planner" -> plannerSection(timer),
        "calibration" -> calibration(points.result(), OptimizerConfig.Default.weights)
      )
      println(report)
      settings.out.foreach(path => Files.writeString(path, report + "\n", StandardCharsets.UTF_8))
    finally native.close()

  /** The access path cost mode chose for the first relation of `sql` (`entity_scan` when no
    * index was a candidate, so no access decision was recorded).
    */
  private def accessPath(db: AdaptiveDatabase, catalog: FileCatalog, sql: String): String =
    db.plan(bind(catalog, sql)).physical.decisions.find(_.operator.startsWith("access ")).fold("entity_scan")(_.choice)

  /** A non-covering balance range (`COUNT(*)` and `SUM(id)` of the rows below a bound) at every
    * fraction of [[SweepFractions]], each run three ways: an index plan (planned with weights
    * that make sequential pages prohibitively expensive and random accesses free, so the index
    * always wins), a sequential plan (rule mode, which ignores indexes) and the plan cost mode
    * chooses. Reports the choice, the time of each plan, the faster path and the chosen plan's
    * time relative to the faster one (`regret`, 1.0 = the right choice); every index and
    * sequential plan becomes a calibration point for the random-access weight.
    */
  private def selectivitySweep(
      db: AdaptiveDatabase,
      catalog: FileCatalog,
      native: NativeDatabase,
      scale: Int,
      timer: Timer,
      points: scala.collection.mutable.Builder[CalibrationPoint, Vector[CalibrationPoint]]
  ): String =
    val forcedIndex = new AdaptiveDatabase(catalog, native, OptimizerConfig(weights = CostWeights(io = 1e7, randomIo = 0)))
    val rows = SweepFractions.map { fraction =>
      val bound = (fraction * BalanceModulus).toLong
      val sql = s"SELECT COUNT(*) AS n, SUM(id) AS s FROM ix_account WHERE balance < $bound"
      val chosen = accessPath(db, catalog, sql)
      val index = timer.measure(drain(native, preparedPlan(forcedIndex, catalog, sql)))
      db.execute("SET optimizer = rule")
      val seq = timer.measure(drain(native, preparedPlan(db, catalog, sql)))
      points += CalibrationPoint(s"sweep_${fraction}_seq", rootEstimate(db, catalog, sql), seq.percentile(0.5))
      db.execute("SET optimizer = cost")
      points += CalibrationPoint(s"sweep_${fraction}_index", rootEstimate(forcedIndex, catalog, sql), index.percentile(0.5))
      val count = db.execute(sql).rows.head.head.asInstanceOf[java.lang.Long].longValue
      val (indexMs, seqMs) = (index.percentile(0.5), seq.percentile(0.5))
      val chosenMs = if chosen.startsWith("index_scan") then indexMs else seqMs
      f"f_$fraction%.4f" -> obj(
        "fraction" -> num(fraction), "balance_below" -> num(bound), "rows" -> num(count),
        "cost_mode_choice" -> str(chosen),
        "index_plan" -> index.json, "seq_plan" -> seq.json,
        "faster" -> str(if indexMs <= seqMs then "index" else "seq"),
        "regret" -> num(chosenMs / math.min(indexMs, seqMs))
      )
    }
    obj(("table_rows" -> num(100_000L * scale)) +: rows*)

  /** Checks that every table holds the rows the scale factor implies; returns the counts. */
  private def verifyRowCounts(db: AdaptiveDatabase, scale: Int): Vector[(String, Long)] =
    BaseRows.map { (table, base, scaled) =>
      val expected = if scaled then base * scale else base
      val actual = db.execute(s"SELECT COUNT(*) FROM $table").rows.head.head.asInstanceOf[java.lang.Long].longValue
      require(actual == expected, s"$table holds $actual rows, expected $expected at scale $scale")
      table -> actual
    }

  /** One query in cost and in rule mode: timing (or the error that stopped it), plan cost,
    * worst q-error and join tree per mode; every plan that ran becomes a calibration point
    * labelled `join_order_<name>_<mode>`.
    */
  private def modes(
      db: AdaptiveDatabase,
      catalog: FileCatalog,
      native: NativeDatabase,
      sql: String,
      timer: Timer,
      name: String,
      points: scala.collection.mutable.Builder[CalibrationPoint, Vector[CalibrationPoint]]
  ): String =
    val results = Vector("cost", "rule").map { mode =>
      db.execute(s"SET optimizer = $mode")
      val tree = joinTree(db.plan(bind(catalog, sql)).optimized)
      val section =
        try
          val prepared = preparedPlan(db, catalog, sql)
          val engine = timer.measure(drain(native, prepared))
          points += CalibrationPoint(s"join_order_${name}_$mode", rootEstimate(db, catalog, sql), engine.percentile(0.5))
          obj(
            "ffi_prepared_plan" -> engine.json,
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

  /** One measured plan for the cost calibration.
    *
    * @param label    workload and mode
    * @param estimate the planner's estimate of the plan's root (cost and its components)
    * @param engineMs median engine time of the prepared plan
    */
  private final case class CalibrationPoint(label: String, estimate: Option[NodeEstimate], engineMs: Double)

  /** The calibration section: every point with its cost components, and the engine time per
    * 1,000 weighted cost units with its spread (max / min) under `weights`.
    */
  private def calibration(points: Vector[CalibrationPoint], weights: CostWeights): String =
    val usable = points.flatMap(p => p.estimate.filter(_.cost > 0).map(p -> _))
    val ratios = usable.map((p, e) => p.label -> p.engineMs * 1000 / e.cost)
    obj(
      "weights" -> obj("cpu" -> num(weights.cpu), "io" -> num(weights.io), "memory" -> num(weights.memory), "hash_row" -> num(weights.hashRow),
        "random_io" -> num(weights.randomIo)),
      "points" -> usable.map { (p, e) =>
        obj(
          "plan" -> str(p.label), "engine_ms" -> num(p.engineMs), "cost" -> num(e.cost),
          "cpu" -> num(e.components.cpu), "io" -> num(e.components.io),
          "memory_bytes" -> num(e.components.memoryBytes), "hash_rows" -> num(e.components.hashRows),
          "random_io" -> num(e.components.randomIo)
        )
      }.mkString("[", ",", "]"),
      "ms_per_1k_cost" -> obj(ratios.map((label, r) => label -> num(r))*),
      "spread" -> num(if ratios.isEmpty then Double.NaN else ratios.map(_._2).max / ratios.map(_._2).min)
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
    { val plans = db.plan(bind(catalog, sql)); PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch) }

  /** Estimated weighted cost of the plan's root. */
  private def rootCost(db: AdaptiveDatabase, catalog: FileCatalog, sql: String): Double =
    rootEstimate(db, catalog, sql).fold(Double.NaN)(_.cost)

  /** The planner's estimate of the plan's root in the session's current mode. */
  private def rootEstimate(db: AdaptiveDatabase, catalog: FileCatalog, sql: String): Option[NodeEstimate] =
    db.plan(bind(catalog, sql)).physical.estimates.get(0)

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
      Timing(samples, last, warm)

  /** Timings of one path.
    *
    * @param inOrder    milliseconds per iteration, in measurement order
    * @param checksum   checksum of the last iteration
    * @param warmupRuns runs before timing started
    */
  private final case class Timing(inOrder: Vector[Double], checksum: Long, warmupRuns: Int):
    /** The samples, sorted. */
    val samples: Vector[Double] = inOrder.sorted

    /** `{"iterations":…,"warmup_runs":…,"p50_ms":…,"p95_ms":…,"p99_ms":…,"mean_ms":…,"min_ms":…,
      * "max_ms":…,"stddev_ms":…,"round_medians_ms":[…],"checksum":…}`.
      */
    def json: String =
      val mean = samples.sum / samples.size
      val stddev = math.sqrt(samples.map(s => (s - mean) * (s - mean)).sum / math.max(2, samples.size))
      obj(
        "iterations" -> num(samples.size), "warmup_runs" -> num(warmupRuns), "p50_ms" -> num(percentile(0.5)),
        "p95_ms" -> num(percentile(0.95)), "p99_ms" -> num(percentile(0.99)), "mean_ms" -> num(mean),
        "min_ms" -> num(samples.head), "max_ms" -> num(samples.last), "stddev_ms" -> num(stddev),
        "round_medians_ms" -> roundMedians.map(num).mkString("[", ",", "]"),
        "checksum" -> num(checksum)
      )

    /** Medians of seven consecutive rounds of the samples in measurement order (their spread
      * shows run-to-run stability, which one median hides).
      */
    def roundMedians: Vector[Double] =
      val size = math.max(1, math.ceil(inOrder.size / 7.0).toInt)
      inOrder.grouped(size).map(round => round.sorted.apply(round.size / 2)).toVector
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
