package io.adb.benchmark

import io.adb.actuator.{ActionKind, ActionState, AdaptiveMode}
import io.adb.bench.{Environment, JitRecord, MeasurementPlan, Report, ReportJson, Runner, SampleStats}
import io.adb.advisor.{Advisor, AdvisorConfig, CreateIndex}
import io.adb.catalog.{Catalog, InMemoryCatalog}
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

/** Milestone 2.5.4.1 workload benchmark over the native engine (the 2.3.0 workloads; since 2.4.0
  * the tables are created in the engine catalog, so every row read goes through its schema).
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
  /** One single-row write that touches no indexed column (Milestone 2.5.4): the statement the
    * write-counter allocation budget is measured on.
    */
  val WriteProbe: String = "UPDATE ix_account SET note = 'probe' WHERE id = 1"
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
    * @param iterations accepted for old command lines and ignored: the measurement plan decides
    * @param scale      scale factor the data was prepared with
    * @param warmupMs   accepted for old command lines and ignored: the measurement plan decides
    * @param out        JSON report path
    */
  final case class Settings(dataDir: Path, nativeLib: Path, iterations: Int, scale: Int, warmupMs: Long, out: Option[Path])

  /** Runs everything and writes the JSON report. */
  def run(settings: Settings): Unit =
    require(Files.isDirectory(settings.dataDir.resolve("rust")), s"${settings.dataDir}/rust not found; prepare it with `workloads --prepare`")
    val native = NativeDatabase.open(settings.nativeLib, settings.dataDir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native)
      val catalog = db.catalog
      require(catalog.entities.isEmpty, s"${settings.dataDir} already has tables; use a freshly prepared directory")
      Ddl.foreach(db.execute)
      val counts = verifyRowCounts(db, settings.scale)
      BaseRows.map(_._1).filterNot(_ == "sk_stale").foreach(table => db.execute(s"ANALYZE $table"))
      val indexBuild = IndexDdl.map { (name, ddl) =>
        val start = System.nanoTime()
        db.execute(ddl)
        name -> obj("sql" -> str(ddl), "ms" -> num((System.nanoTime() - start) / 1e6))
      }
      db.execute("ANALYZE ix_account") // index statistics for the cost model
      val plan = MeasurementPlan.fromEnv(durable = false)
      val timer = Timer(plan)
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
        points += CalibrationPoint(name, rootEstimate(db, catalog, sql), engine.medianMs)
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
        points += CalibrationPoint(s"scan_$name", rootEstimate(db, catalog, sql), engine.medianMs)
      }
      val document = new Report("jvm-workloads", "2.5.4.1", plan, Environment.capture())
      Vector(
        "host" -> obj(
          "os" -> str(System.getProperty("os.name")), "arch" -> str(System.getProperty("os.arch")),
          "cpu" -> str(cpuModel()), "cpus" -> num(Runtime.getRuntime.availableProcessors()),
          "java" -> str(System.getProperty("java.version")), "max_heap_mib" -> num(Runtime.getRuntime.maxMemory() / 1048576)
        ),
        "setup" -> obj("scale" -> num(settings.scale), "iterations" -> num(plan.totalSamples.toLong), "warmup_ms" -> num(plan.minSampleNs / 1_000_000L),
          "rows" -> obj(counts.map((t, n) => t -> num(n))*)),
        "index_build" -> obj(indexBuild*),
        "workloads" -> obj(workloads*),
        "selectivity_sweep" -> sweep,
        "join_order" -> obj(joinOrder*),
        "estimation" -> estimationSection(db),
        "planner" -> plannerSection(timer),
        "calibration" -> calibration(points.result(), OptimizerConfig.Default.weights),
        "telemetry" -> telemetrySection(db),
        "advisor" -> advisorSection(db),
        "actuator" -> actuatorSection(db)
      ).foreach((key, value) => document.set(key, value))
      val report = document.finish()
      println(report)
      settings.out.foreach(path => Files.writeString(path, report + "\n", StandardCharsets.UTF_8))
    finally native.close()

  /** Telemetry cost of workload C through the whole SQL path (Milestone 2.5.2.1): the bytes the
    * calling thread allocates per execution with telemetry off, skipped by the sampler and
    * captured, and the rows read a capture reports. Since 2.5.4 it also reports the bytes a
    * single-row write allocates with the per-entity write counters off and on.
    *
    * Allocation, unlike time, does not depend on the machine's load, so
    * `scripts/check-benchmarks.py` can hold the numbers to a budget: a skipped query adds
    * (almost) nothing and a captured one a few hundred bytes, not the kilobyte of a decoded
    * profile document.
    */
  private def telemetrySection(db: AdaptiveDatabase): String =
    val runs = 2000
    val threads = java.lang.management.ManagementFactory.getThreadMXBean.asInstanceOf[com.sun.management.ThreadMXBean]
    val policy = db.telemetry.policy
    db.telemetry.policy = io.adb.telemetry.TelemetrySamplingPolicy(fastSampleEvery = 1 << 20, slowQueryNanos = Long.MaxValue)
    /** Bytes allocated per execution of workload C in `mode`, after a warm-up. */
    def allocated(mode: String): Double =
      db.execute(s"SET telemetry = $mode")
      for _ <- 0 until 1000 do db.execute(WorkloadC)
      val before = threads.getCurrentThreadAllocatedBytes
      var i = 0
      while i < runs do { db.execute(WorkloadC); i += 1 }
      (threads.getCurrentThreadAllocatedBytes - before).toDouble / runs
    try
      // The JIT removes allocations as it compiles, so a later measurement is lower for reasons
      // unrelated to telemetry: each mode is measured in six rounds and compared by its smallest
      // figure, when all of them are equally warm.
      val figures = (0 until 6).map(_ => (allocated("off"), allocated("sampled"), allocated("full")))
      val (off, skip, capture) = (figures.map(_._1).min, figures.map(_._2).min, figures.map(_._3).min)
      db.telemetry.drainNow()
      val perExecution = db.telemetry.allStats().map(s => s.totalRowsRead.toDouble / math.max(1L, s.executions))
      // The write counters (2.5.4) are measured the same way on one single-row update.
      def writeAllocated(mode: String): Double =
        db.execute(s"SET telemetry = $mode")
        for _ <- 0 until 1000 do db.execute(WriteProbe)
        val before = threads.getCurrentThreadAllocatedBytes
        var i = 0
        while i < runs do { db.execute(WriteProbe); i += 1 }
        (threads.getCurrentThreadAllocatedBytes - before).toDouble / runs
      val writeFigures = (0 until 6).map(_ => (writeAllocated("off"), writeAllocated("full")))
      val (writeOff, writeOn) = (writeFigures.map(_._1).min, writeFigures.map(_._2).min)
      obj(
        "write_query" -> str("update_single_row"),
        "write_alloc_bytes_off" -> num(writeOff),
        "write_alloc_bytes_counted" -> num(writeOn),
        "write_alloc_extra_counted" -> num(writeOn - writeOff),
        "write_statements_counted" -> num(db.telemetry.writes.all().map(_.statements).sum.toDouble),
        "query" -> str("C_covering_index_eq"),
        "runs" -> num(runs.toLong),
        "alloc_bytes_off" -> num(off),
        "alloc_bytes_skip" -> num(skip),
        "alloc_bytes_capture" -> num(capture),
        "alloc_extra_skip" -> num(skip - off),
        "alloc_extra_capture" -> num(capture - off),
        "captured_rows_read_per_execution" -> num(perExecution.headOption.getOrElse(-1.0))
      )
    finally
      db.execute("SET telemetry = off")
      db.telemetry.policy = policy

  /** What the advisor does with workload E (Milestone 2.5.3): `COUNT(*)` over `ix_account` filtered
    * by the unindexed `region` reads every row to return one, so after three evaluations (the
    * hysteresis of the defaults) the advisor must recommend an index on `region`, and it must not
    * create it: the number of indexes is the same before and after.
    *
    * The figures are counts, not times, so `scripts/check-benchmarks.py` can hold them exactly;
    * the cost of one evaluation is reported for information.
    */
  private def advisorSection(db: AdaptiveDatabase): String =
    val indexesBefore = db.execute("SHOW INDEXES").rows.size
    val advisor = Advisor.forTelemetry(db.telemetry, AdvisorConfig())
    db.execute("SET telemetry = full")
    try
      val evaluateNanos = Vector.newBuilder[Long]
      def evaluate() =
        db.telemetry.drainNow()
        val start = System.nanoTime()
        val report = advisor.evaluate(db.physicalDesign())
        evaluateNanos += System.nanoTime() - start
        report
      evaluate() // baseline: the counters of the earlier sections are not one window
      val reports = (0 until 3).map { _ =>
        for _ <- 0 until 20 do db.execute(WorkloadE)
        evaluate()
      }
      val indexes = reports.last.recommendations.collect { case c: CreateIndex => c }
      obj(
        "query" -> str("E_seq_scan_region_eq"),
        "evaluations" -> num(reports.size + 1L),
        "recommended_after_two" -> num(reports(1).recommendations.count(_.isInstanceOf[CreateIndex]).toLong),
        "recommended_indexes" -> num(indexes.size.toLong),
        "index_sql" -> str(indexes.headOption.flatMap(_.sql).getOrElse("")),
        "indexes_before" -> num(indexesBefore.toLong),
        "indexes_after" -> num(db.execute("SHOW INDEXES").rows.size.toLong),
        "audit_entries" -> num(advisor.auditLog.size.toLong),
        "evaluate_us_min" -> num(evaluateNanos.result().min / 1000.0)
      )
    finally db.execute("SET telemetry = off")

  /** What the 2.5.4 actuator does, end to end, on two small tables (Milestone 2.5.4).
    *
    * `act_orders` is filtered on a selective column (20 customers over 5 000 rows), so the index the
    * advisor builds cuts the rows a query reads twentyfold and must be kept. `act_events` is filtered
    * on a column that is the same in every row, so its index cannot help and must be rolled back, and
    * the same target must then be refused during the rollback cooldown. The tables are small on
    * purpose: `CREATE INDEX` stalls the database for the whole build (about 30 us per row).
    *
    * The figures are counts and exact engine counters, not times, so `scripts/check-benchmarks.py`
    * holds them exactly: actions applied, kept, rolled back and rejected, the rows read per execution
    * before and after, the user's indexes unchanged, and nothing done while the mode is off or
    * advisory. The mean cost of one controller tick is reported for information.
    */
  private def actuatorSection(db: AdaptiveDatabase): String =
    val rows = 5000
    db.execute("CREATE TABLE act_orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL)")
    db.execute("CREATE TABLE act_events (id BIGINT PRIMARY KEY, flag BIGINT NOT NULL, amount BIGINT NOT NULL)")
    for i <- 1 to rows do
      db.execute(s"INSERT INTO act_orders (id, customer_id, amount) VALUES ($i, ${1 + i % 20}, ${i * 3})")
      db.execute(s"INSERT INTO act_events (id, flag, amount) VALUES ($i, 1, ${i * 3})")
    val byCustomer = (n: Int) => s"SELECT id, amount FROM act_orders WHERE customer_id = ${1 + n % 20}"
    val byFlag = (n: Int) => s"SELECT id FROM act_events WHERE flag = 1 AND amount = ${3 * (1 + n % rows)}"
    val mixed = (n: Int) => if n % 2 == 0 then byFlag(n) else byCustomer(n)
    def indexes(): Vector[String] = db.execute("SHOW INDEXES").rows.map(_.head.toString)
    def userIndexes(): Int = indexes().count(!_.startsWith("adv_"))
    def tick(): Unit =
      db.telemetry.drainNow()
      db.controller.tick(db.physicalDesign())
    def round(sql: Int => String): Unit =
      for n <- 1 to 100 do db.execute(sql(n))
      tick()
    /** Cumulative (rows read, executions) of every shape that reads the table of `sql`. */
    def rowsRead(sql: String): (Long, Long) =
      db.telemetry.drainNow()
      val entity = entityId(db, sql)
      val mine = db.telemetry.allStats().filter(st => db.telemetry.registry.get(st.shapeId).exists(_.entities.contains(entity)))
      (mine.map(_.totalRowsRead).sum, mine.map(_.executions).sum)

    val userBefore = userIndexes()
    db.execute("SET telemetry = full")
    try
      // Off and advisory: nothing may happen however long the workload runs.
      for _ <- 1 to 6 do round(mixed)
      db.controller.mode = AdaptiveMode.Advisory
      for _ <- 1 to 6 do round(mixed)
      val idleActions = db.controller.actions.size + db.controller.log.size
      // auto_safe, from a fresh model so the hysteresis counts three evaluations afresh.
      db.advisor.reset()
      db.controller.mode = AdaptiveMode.AutoSafe
      round(mixed)                                        // baseline of the model
      var built = false
      var rowsBefore = (0L, 0L)
      var rounds = 0
      while !built && rounds < 12 do
        round(mixed); rounds += 1
        built = indexes().contains("adv_act_orders_customer_id")
        if built then rowsBefore = rowsRead(byCustomer(1))
      val atBuild = rowsRead(byCustomer(1))
      for _ <- 1 to 8 do round(mixed)                     // verification windows, then the rollback cooldown
      val after = rowsRead(byCustomer(1))
      val perExecBefore = rowsBefore._1.toDouble / math.max(1L, rowsBefore._2)
      val perExecAfter = (after._1 - atBuild._1).toDouble / math.max(1L, after._2 - atBuild._2)
      val actions = db.controller.actions
      val events = db.controller.log.entries.map(_.event.label)
      val tickNanos = (0 until 20).map { _ =>
        db.telemetry.drainNow()
        val start = System.nanoTime()
        db.controller.tick(db.physicalDesign())
        System.nanoTime() - start
      }
      obj(
        "table_rows" -> num(rows.toLong),
        "idle_actions" -> num(idleActions.toLong),
        "actions_applied" -> num(events.count(_ == "applied").toLong),
        "actions_kept" -> num(actions.count(a => a.kind == ActionKind.CreateIndex && a.state == ActionState.Kept).toLong),
        "actions_rolled_back" -> num(actions.count(a => a.kind == ActionKind.CreateIndex && a.state == ActionState.RolledBack).toLong),
        "actions_rejected" -> num(events.count(_ == "rejected").toLong),
        "actions_failed" -> num(events.count(_ == "failed").toLong),
        "rows_read_per_execution_before" -> num(perExecBefore),
        "rows_read_per_execution_after" -> num(perExecAfter),
        "user_indexes_before" -> num(userBefore.toLong),
        "user_indexes_after" -> num(userIndexes().toLong),
        "advisor_indexes_after" -> num(indexes().count(_.startsWith("adv_")).toLong),
        "tick_us_mean" -> num(tickNanos.sum.toDouble / tickNanos.size / 1000.0)
      )
    finally
      db.controller.mode = AdaptiveMode.Off
      db.execute("SET telemetry = off")

  /** The id of the entity `sql` reads (its single table). */
  private def entityId(db: AdaptiveDatabase, sql: String): Long =
    val table = "(?i)\\bfrom\\s+(\\w+)".r.findFirstMatchIn(sql).get.group(1)
    db.catalog.entities.find(_.name.equalsIgnoreCase(table)).get.id.value

  /** The access path cost mode chose for the first relation of `sql` (`entity_scan` when no
    * index was a candidate, so no access decision was recorded).
    */
  private def accessPath(db: AdaptiveDatabase, catalog: Catalog, sql: String): String =
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
      catalog: Catalog,
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
      points += CalibrationPoint(s"sweep_${fraction}_seq", rootEstimate(db, catalog, sql), seq.medianMs)
      db.execute("SET optimizer = cost")
      points += CalibrationPoint(s"sweep_${fraction}_index", rootEstimate(forcedIndex, catalog, sql), index.medianMs)
      val count = db.execute(sql).rows.head.head.asInstanceOf[java.lang.Long].longValue
      val (indexMs, seqMs) = (index.medianMs, seq.medianMs)
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
      catalog: Catalog,
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
          points += CalibrationPoint(s"join_order_${name}_$mode", rootEstimate(db, catalog, sql), engine.medianMs)
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
      s"${shape}_$n" -> obj("relations" -> num(n), "p50_us" -> num(timing.medianMs * 1000), "p99_us" -> num(timing.p99Ms * 1000), "method" -> str(method))
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
  private def planOnly(db: AdaptiveDatabase, catalog: Catalog, sql: String): Long =
    preparedPlan(db, catalog, sql).length.toLong

  /** Physical plan JSON of `sql` in the session's current mode. */
  private def preparedPlan(db: AdaptiveDatabase, catalog: Catalog, sql: String): String =
    { val plans = db.plan(bind(catalog, sql)); PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch) }

  /** Estimated weighted cost of the plan's root. */
  private def rootCost(db: AdaptiveDatabase, catalog: Catalog, sql: String): Double =
    rootEstimate(db, catalog, sql).fold(Double.NaN)(_.cost)

  /** The planner's estimate of the plan's root in the session's current mode. */
  private def rootEstimate(db: AdaptiveDatabase, catalog: Catalog, sql: String): Option[NodeEstimate] =
    db.plan(bind(catalog, sql)).physical.estimates.get(0)

  /** Worst q-error reported by EXPLAIN ANALYZE. */
  private def maxQError(db: AdaptiveDatabase, sql: String): Double =
    val text = db.execute("EXPLAIN ANALYZE " + sql).rows.map(_.head.toString).mkString("\n")
    raw"max q-error ([0-9.]+)".r.findFirstMatchIn(text).fold(Double.NaN)(_.group(1).toDouble)

  /** Binds a SELECT. */
  private def bind(catalog: Catalog, sql: String): BoundSelect =
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

  /** Timed runs on the `adb-bench` harness: warm-up, calibration, then batches of samples.
    *
    * @param plan the measurement plan (CPU-bound cases use the release plan)
    */
  private final case class Timer(plan: MeasurementPlan):
    /** Measures `body` (which returns a checksum) according to the plan. */
    def measure(body: => Long): Timing =
      val measured = Runner.measure(plan)(body)
      Timing(SampleStats.fromMeasured(measured), measured.last, measured.jit, plan)

  /** Timings of one path.
    *
    * @param stats    the statistics of the samples (nanoseconds per invocation)
    * @param checksum checksum of the last iteration
    * @param jit      the JIT and GC record of the measurement
    * @param plan     the plan the samples were taken with
    */
  private final case class Timing(stats: SampleStats, checksum: Long, jit: JitRecord, plan: MeasurementPlan):
    /** The headline, the median of the batch medians, in milliseconds. */
    def medianMs: Double = stats.medianNs / 1e6

    /** The 99th percentile in milliseconds. */
    def p99Ms: Double = stats.p99Ns / 1e6

    /** `{"iterations":…,"warmup_runs":…,"p50_ms":…,…,"round_medians_ms":[…],"checksum":…,"timing":{…},"jit":{…}}`:
      * the keys of the earlier reports (`p50_ms` is now the median of medians) plus the harness's.
      */
    def json: String =
      ReportJson.caseJson(
        stats,
        Some(jit),
        Seq(
          "iterations" -> num(plan.totalSamples.toLong * stats.iterationsPerSample),
          "warmup_runs" -> num(plan.warmupInvocations.toLong),
          "checksum" -> num(checksum)
        )
      )

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
