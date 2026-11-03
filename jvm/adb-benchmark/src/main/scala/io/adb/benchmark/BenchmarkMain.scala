package io.adb.benchmark

import io.adb.catalog.{Catalog, InMemoryCatalog}
import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import io.adb.logical.{Binder, BoundCreateTable, BoundSelect, LogicalPlanner}
import io.adb.optimizer.RuleOptimizer
import io.adb.plancache.StatementKey
import io.adb.gateway.QueryShapeExtractor
import io.adb.advisor.{Advisor, AdvisorConfig, EntityDesign, PhysicalDesign}
import io.adb.telemetry.*
import io.adb.physical.{PhysicalPlanner, PlanJsonEncoder}
import io.adb.sql.SqlParser

import io.adb.ffm.NativeQuery

import java.nio.file.{Files, Path}
import java.util.Optional
import scala.annotation.tailrec

/** JVM benchmark (Milestone 2.5.3) separating JVM planning cost from native end-to-end execution.
  *
  * Planner stages are timed for a point lookup and for a relational query (join, GROUP BY,
  * ORDER BY + LIMIT); the optional gateway section runs both through Java FFM against the
  * native engine. `--workloads` runs the workload benchmark (2.3.0 workloads, engine catalog since 2.4.0) instead
  * ([[WorkloadBenchmark]]).
  */
object BenchmarkMain:
  /** Command-line options.
    *
    * @param iterations        iterations of each planner micro-benchmark
    * @param gatewayIterations iterations of the SQL-to-native gateway benchmark
    * @param dataDir           database directory for the gateway benchmark
    * @param nativeLib         path of the native library for the gateway benchmark
    * @param workloads         run the workload benchmark (needs `dataDir` and `nativeLib`)
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
    println(s"Adaptive DB 2.5.3 JVM benchmark iterations=${cfg.iterations} gatewayIterations=${cfg.gatewayIterations}")
    val catalog = new InMemoryCatalog
    createTables(catalog)
    benchmarkPlanner("", PointSql, catalog, cfg.iterations)
    benchmarkPlanner("_relational", RelationalSql, catalog, cfg.iterations)
    benchmarkKey(cfg.iterations)
    benchmarkTelemetryParts(catalog, cfg.iterations)
    benchmarkAdvisor(cfg.iterations)
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
    val wireNs = time(iterations) { PlanJsonEncoder.encode(physical.plan, 0) }
    val totalNs = time(iterations) { pipeline(parser, binder, optimizer, sql) }

    report(s"sql_parse$suffix", iterations, parseNs)
    report(s"bind_slots$suffix", iterations, bindNs)
    report(s"logical_plan$suffix", iterations, logicalNs)
    report(s"rule_optimize$suffix", iterations, optimizeNs)
    report(s"physical_plan$suffix", iterations, physicalNs)
    report(s"plan_json_encode$suffix", iterations, wireNs)
    report(s"planner_pipeline$suffix", iterations, totalNs)

  /** Times building the plan-cache key of both benchmark statements: the lexing every cached
    * statement pays on every execution, hit or miss.
    */
  private def benchmarkKey(iterations: Int): Unit =
    for _ <- 0 until math.min(1000, iterations) do StatementKey.of(PointSql, "cost")
    report("plan_cache_key_point", iterations, time(iterations) { StatementKey.of(PointSql, "cost") })
    report("plan_cache_key_relational", iterations, time(iterations) { StatementKey.of(RelationalSql, "cost") })

  /** Times the pieces of the telemetry hot path separately, so a regression can be traced to one
    * of them: the sampling decision, building the shape (done once per plan, not per execution),
    * building the event, and the queue offer.
    */
  private def benchmarkTelemetryParts(catalog: Catalog, iterations: Int): Unit =
    val bound = new Binder(catalog).bind(new SqlParser().parse(PointSql)).asInstanceOf[BoundSelect]
    val policy = TelemetrySamplingPolicy(fastSampleEvery = 128)
    val runtime = new TelemetryRuntime(capacity = 1 << 16, startWorker = false)
    runtime.mode = TelemetryMode.Sampled
    val shape = QueryShapeExtractor.extract(bound)
    runtime.registry.register(shape)
    val n = iterations * 10
    for _ <- 0 until math.min(10000, n) do runtime.decide(shape.id, 1000, failed = false)
    report("telemetry_sampling_policy", n, time(n) { policy.decide(12345L, 1000, failed = false, knownShape = true) })
    report("telemetry_sampling_decide", n, time(n) { runtime.decide(shape.id, 1000, failed = false) })
    report("telemetry_shape_extract", iterations, time(iterations) { QueryShapeExtractor.extract(bound) })
    val cachedShape = shape
    report("telemetry_cached_shape_id", n, time(n) { cachedShape.id })
    report("telemetry_event_construct", n, time(n) { QueryTelemetry(shape.id, 1, 1, 1, 10, 1000, true, false) })
    val event = QueryTelemetry(shape.id, 1, 1, 1, 10, 1000, true, false)
    val offerNs = time(n) { runtime.offer(event); if (runtime.metrics.queueSize >= 60000) runtime.drainNow() }
    report("telemetry_queue_offer_with_drain", n, offerNs)
    // A profile of the size a point lookup or a small join produces (the parse is paid by captured queries only).
    val profile =
      """{"peak_memory_bytes":2048,"memory_limit_bytes":268435456,"root":{"node_id":0,"operator":"project","rows_out":1,"batches_out":1,"elapsed_us":12,"counters":{"rows_in":1},""" +
        """"children":[{"node_id":1,"operator":"filter","rows_out":1,"batches_out":1,"elapsed_us":8,"children":[{"node_id":2,"operator":"entity_scan","rows_out":1000,"batches_out":1,"elapsed_us":40,"counters":{"pages":12}}]}]}}"""
    report("telemetry_profile_rows_scanned", iterations, time(iterations) { io.adb.gateway.ProfileRenderer.rowsScanned(profile) })

  /** Times one advisor evaluation over a workload of 50 shapes on 5 tables (Milestone 2.5.3):
    * closing a window, the four detectors and the hysteresis. The advisor runs off the query path,
    * every few seconds, so this is the cost of one tick and not a per-query cost.
    */
  private def benchmarkAdvisor(iterations: Int): Unit =
    val tables = (1L to 5L).toVector
    val shapes = (0 until 50).map { i =>
      val table = tables(i % tables.size)
      QueryShape(Vector(table), Vector(FieldRef(table, 2L + i / 5)), Vector.empty, Vector.empty, i % 3 == 0, false)
    }.toVector
    val byId = shapes.map(s => s.id -> s).toMap
    val design = PhysicalDesign.of(tables.map(t =>
      EntityDesign(t, s"t$t", (1L to 12L).map(f => f -> s"c$f").toMap, 1_000_000L, None, Vector.empty)))
    var tick = 0L
    // Every call of `stats` reports more executions, so each evaluation closes a window with work in it.
    val stats = () =>
      tick += 1
      shapes.map(s => ShapeStats(s.id, tick * 100, 0L, 0L, tick * 100 * 2_000_000L, 4_000_000L, tick * 100 * 10, tick * 100 * 5_000, 1.0))
    val advisor = new Advisor(stats, byId.get, AdvisorConfig())
    val n = math.max(100, iterations)
    for _ <- 0 until math.min(1000, n) do advisor.evaluate(design)
    report("advisor_evaluate_50_shapes", n, time(n) { advisor.evaluate(design) })

  /** Runs the full planning pipeline for one SELECT and returns the native plan JSON. */
  private def pipeline(parser: SqlParser, binder: Binder, optimizer: RuleOptimizer, sql: String): String =
    val bound = binder.bind(parser.parse(sql)).asInstanceOf[BoundSelect]
    val logical = LogicalPlanner.plan(bound)
    val optimized = optimizer.optimize(logical)
    PlanJsonEncoder.encode(PhysicalPlanner.plan(optimized).plan, 0)

  /** Times end-to-end SQL execution through FFM: point lookups, a filtered scan with LIMIT,
    * and the relational query over 1,000 accounts and 10,000 transfers.
    */
  private def benchmarkGateway(iterations: Int, dataDir: Path, nativeLib: Path): Unit =
    Files.createDirectories(dataDir)
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native, Some(dataDir.resolve("catalog.properties")))
      if db.catalog.entity("account").isEmpty then
        Ddl.foreach(db.execute)
        for i <- 1 to 1000 do db.execute(s"INSERT INTO account VALUES ($i, ${i * 10L}, 'owner-${i % 50}');")
        for i <- 1 to 10000 do db.execute(s"INSERT INTO transfer VALUES ($i, ${1 + i % 1000}, ${i % 97});")

      for _ <- 0 until math.min(100, iterations) do db.execute("SELECT * FROM account WHERE id = 500;")
      val pointNs = time(iterations) { db.execute("SELECT * FROM account WHERE id = 500;") }
      val scanIterations = math.max(1, iterations / 10)
      val scanNs = time(scanIterations) { db.execute("SELECT id, balance FROM account WHERE balance >= 9000 LIMIT 100;") }
      val relationalIterations = math.max(1, iterations / 100)
      val relationalNs = time(relationalIterations) { db.execute(RelationalSql) }
      // Plan cache (2.5.1): the same statements with the cache off (every execution is parsed,
      // bound and planned, as in 2.5.0) and on (after admission, a hit). The difference is the
      // planning cost the cache removes; the remainder is the engine and the fixed gateway cost.
      val rel = math.max(1, iterations / 100)
      db.execute("SET plan_cache = off")
      for _ <- 0 until math.min(100, iterations) do db.execute("SELECT * FROM account WHERE id = 500;")
      val pointOff = time(iterations) { db.execute("SELECT * FROM account WHERE id = 500;") }
      val relOff = time(rel) { db.execute(RelationalSql) }
      db.execute("SET plan_cache = on")
      for _ <- 0 until math.min(100, iterations) do db.execute("SELECT * FROM account WHERE id = 500;")
      db.execute(RelationalSql); db.execute(RelationalSql)
      val pointOn = time(iterations) { db.execute("SELECT * FROM account WHERE id = 500;") }
      val relOn = time(rel) { db.execute(RelationalSql) }
      report("gateway_point_lookup_cache_off", iterations, pointOff)
      report("gateway_point_lookup_cache_on", iterations, pointOn)
      report("gateway_join_aggregate_cache_off", rel, relOff)
      report("gateway_join_aggregate_cache_on", rel, relOn)
      // Telemetry (2.5.2) on the cached point lookup: off (one volatile read), on but skipped
      // (sampler only) and captured (compact event; the worker thread drains it).
      val telemetryRuns = iterations * 2
      val pointSql = "SELECT * FROM account WHERE id = 500;"
      /** Time of `telemetryRuns` cached point lookups in `mode`, after a warm-up. */
      def measure(mode: String): Long =
        db.execute(s"SET telemetry = $mode")
        for _ <- 0 until 1000 do db.execute(pointSql)
        time(telemetryRuns) { db.execute(pointSql) }
      db.telemetry.policy = TelemetrySamplingPolicy(fastSampleEvery = 1 << 20, slowQueryNanos = Long.MaxValue)
      // The modes are measured in alternating rounds and the median of each is reported, because
      // a single pass is dominated by drift (JIT, CPU frequency, the other threads of the VM).
      val rounds = 9
      val off, skip, capture = Array.fill(rounds)(0L)
      val modes = Array("off", "sampled", "full")
      val results = Array(off, skip, capture)
      // The order rotates every round: a fixed order would charge whatever the machine does
      // between the first and the last measurement (a frequency step, a noisy neighbour) to the
      // mode that always runs last.
      for r <- 0 until rounds do
        for k <- 0 until 3 do
          val m = (r + k) % 3
          results(m)(r) = measure(modes(m))
      db.execute("SET telemetry = off")
      def median(a: Array[Long]): Long = a.sorted.apply(a.length / 2)
      val (tOff, tSkip, tCapture) = (median(off), median(skip), median(capture))
      report("gateway_point_telemetry_off", telemetryRuns, tOff)
      report("gateway_point_telemetry_skip", telemetryRuns, tSkip)
      report("gateway_point_telemetry_capture", telemetryRuns, tCapture)
      // The fastest round of each mode is the least disturbed by the machine, so it is the
      // steadier estimate of the cost the mode itself adds; the median is kept for continuity.
      val (mOff, mSkip, mCapture) = (off.min, skip.min, capture.min)
      println(f"telemetry overhead vs off (fastest of $rounds rounds): skip ${100.0 * (mSkip - mOff) / mOff}%.1f%% capture ${100.0 * (mCapture - mOff) / mOff}%.1f%%")
      println(f"telemetry overhead vs off (median of $rounds rounds, rotating order): skip ${100.0 * (tSkip - tOff) / tOff}%.1f%% capture ${100.0 * (tCapture - tOff) / tOff}%.1f%%; dropped=${db.telemetry.metrics.dropped}")
      benchmarkCaptureBreakdown(db, native, iterations)
      println(f"plan cache: hit ratio ${db.planCacheStats.hitRatio}%.3f entries=${db.planCacheStats.entries} bytes=${db.planCacheStats.bytes}")
      // The native calls a cache hit makes before it runs the plan (2.5.2 baseline for the
      // telemetry budgets): the schema epoch, the statistics generation of one table, the
      // commit head for the session-token check.
      val entityId = db.catalog.entity("account").get.id.value
      val nativeIters = iterations * 10
      report("native_call_schema_epoch", nativeIters, time(nativeIters) { native.currentSchemaEpoch() })
      report("native_call_statistics_generation", nativeIters, time(nativeIters) { native.statisticsGeneration(entityId) })
      report("native_call_latest_commit_seq", nativeIters, time(nativeIters) { native.latestCommitSeq() })
      report("gateway_point_lookup_sql_to_rust", iterations, pointNs)
      report("gateway_scan_filter_limit_sql_to_rust", scanIterations, scanNs)
      report("gateway_join_aggregate_top_k_sql_to_rust", relationalIterations, relationalNs)
    finally native.close()

  /** Splits the cost of a captured query into the steps the capture path adds to the engine run
    * (Milestone 2.5.3): the native execution alone, the same plus fetching the profile document,
    * and the same plus scanning it for the rows read. It also reports the bytes the calling thread
    * allocates per execution with telemetry off, skipped and captured.
    *
    * The steps are timed in alternating rounds and the median of each is reported, as for the
    * telemetry modes, so a drift of the machine does not look like a difference between steps.
    */
  private def benchmarkCaptureBreakdown(db: AdaptiveDatabase, native: NativeDatabase, iterations: Int): Unit =
    val sql = "SELECT * FROM account WHERE id = 500;"
    val bound = new Binder(db.catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    val plans = db.plan(bound)
    val json = PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch)
    val snapshot = Optional.empty[java.lang.Long]()
    /** Runs the plan to the end of the stream and returns the number of rows. */
    def drain(query: NativeQuery): Int =
      var rows = 0
      var next = query.nextBatch()
      while next.isPresent do
        rows += next.get().rowCount()
        next = query.nextBatch()
      rows
    val runs = iterations * 2
    def execOnly(): Int =
      val query = native.execute(json, snapshot)
      try drain(query) finally query.close()
    def execProfile(): Int =
      val query = native.execute(json, snapshot)
      try drain(query) + query.profileJson().length finally query.close()
    def execProfileScan(): Long =
      val query = native.execute(json, snapshot)
      try
        drain(query)
        io.adb.gateway.ProfileRenderer.rowsScanned(query.profileJson())
      finally query.close()
    for _ <- 0 until 2000 do { execOnly(); execProfile(); execProfileScan() }
    val rounds = 9
    val a, b, c = Array.fill(rounds)(0L)
    for r <- 0 until rounds do
      // Rotating order, as for the telemetry modes.
      for k <- 0 until 3 do
        (r + k) % 3 match
          case 0 => a(r) = time(runs) { execOnly() }
          case 1 => b(r) = time(runs) { execProfile() }
          case _ => c(r) = time(runs) { execProfileScan() }
    def median(x: Array[Long]): Long = x.sorted.apply(x.length / 2)
    val (tExec, tProfile, tScan) = (median(a), median(b), median(c))
    report("capture_native_execute_only", runs, tExec)
    report("capture_native_execute_profile", runs, tProfile)
    report("capture_native_execute_profile_scan", runs, tScan)
    println(f"capture steps (ns/op): profile fetch ${(tProfile - tExec).toDouble / runs}%.0f, row scan ${(tScan - tProfile).toDouble / runs}%.0f")

    val threads = java.lang.management.ManagementFactory.getThreadMXBean.asInstanceOf[com.sun.management.ThreadMXBean]
    /** Bytes the calling thread allocates per execution of `sql` in telemetry `mode`. */
    def allocated(mode: String): Double =
      db.execute(s"SET telemetry = $mode")
      for _ <- 0 until 2000 do db.execute(sql)
      val before = threads.getCurrentThreadAllocatedBytes
      var i = 0
      while i < runs do { db.execute(sql); i += 1 }
      (threads.getCurrentThreadAllocatedBytes - before).toDouble / runs
    val policy = db.telemetry.policy
    db.telemetry.policy = TelemetrySamplingPolicy(fastSampleEvery = 1 << 20, slowQueryNanos = Long.MaxValue)
    val (off, skip, capture) = (allocated("off"), allocated("sampled"), allocated("full"))
    db.execute("SET telemetry = off")
    db.telemetry.policy = policy
    println(f"gateway_point_alloc_bytes_off      $off%10.0f B/op")
    println(f"gateway_point_alloc_bytes_skip     $skip%10.0f B/op (+${skip - off}%.0f vs off)")
    println(f"gateway_point_alloc_bytes_capture  $capture%10.0f B/op (+${capture - off}%.0f vs off)")

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
