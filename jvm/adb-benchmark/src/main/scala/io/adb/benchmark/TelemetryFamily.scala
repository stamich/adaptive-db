package io.adb.benchmark

import io.adb.bench.*
import io.adb.benchmark.FamilyCommon.*
import io.adb.ffm.NativeQuery
import io.adb.gateway.{AdaptiveDatabase, QueryShapeExtractor}
import io.adb.logical.{Binder, BoundSelect}
import io.adb.physical.PlanJsonEncoder
import io.adb.sql.SqlParser
import io.adb.telemetry.*

import java.util.Optional

/** The `telemetry` family (Milestone 2.5.4.1): what query telemetry costs.
  *
  * Three kinds of case. **Parts** of the hot path timed separately (the sampling decision, the shape,
  * the event, the queue), so a regression can be traced to one of them. **Modes** of the cached point
  * lookup (telemetry off, sampled but skipped, captured), measured *interleaved* so a drift of the
  * machine is not charged to one mode, with the capture steps (execute only, plus fetching the
  * profile, plus scanning it) measured the same way. **Allocation** per execution in each mode and for
  * a single-row write with the per-entity write counters off and on: allocation, unlike time, does
  * not depend on the load of the machine, so it is the figure `check-benchmarks.py` holds to a budget.
  */
object TelemetryFamily:
  /** Runs the family and returns the report JSON. */
  def run(settings: FamilySettings): String =
    val plan = cpuPlan
    val catalog = new io.adb.catalog.InMemoryCatalog
    createTables(catalog)
    val parts = partCases(catalog, plan)
    val engine = (settings.dataDir, settings.nativeLib) match
      case (Some(data), Some(lib)) => engineCases(data, lib, plan)
      case _ => engineNames.map(_ -> skipped("no --data and --native-lib given"))
    finish("telemetry", plan, settings, Seq.empty, parts ++ engine)

  /** Names of the cases that need the engine. */
  private val engineNames = Vector(
    "gateway_point_telemetry_off", "gateway_point_telemetry_skip", "gateway_point_telemetry_capture",
    "capture_native_execute_only", "capture_native_execute_profile", "capture_native_execute_profile_scan",
    "alloc_point_off", "alloc_point_skip", "alloc_point_capture", "alloc_write_off", "alloc_write_counted"
  )

  /** The hot-path parts, each on its own. */
  private def partCases(catalog: io.adb.catalog.Catalog, plan: MeasurementPlan): Vector[(String, String)] =
    val bound = new Binder(catalog).bind(new SqlParser().parse(PointSql)).asInstanceOf[BoundSelect]
    val policy = TelemetrySamplingPolicy(fastSampleEvery = 128)
    val runtime = new TelemetryRuntime(capacity = 1 << 16, startWorker = false)
    runtime.mode = TelemetryMode.Sampled
    val shape = QueryShapeExtractor.extract(bound)
    runtime.registry.register(shape)
    val event = QueryTelemetry(shape.id, 1, 1, 1, 10, 1000, true, false)
    val profile =
      """{"peak_memory_bytes":2048,"memory_limit_bytes":268435456,"root":{"node_id":0,"operator":"project","rows_out":1,"batches_out":1,"elapsed_us":12,"counters":{"rows_in":1},""" +
        """"children":[{"node_id":1,"operator":"filter","rows_out":1,"batches_out":1,"elapsed_us":8,"children":[{"node_id":2,"operator":"entity_scan","rows_out":1000,"batches_out":1,"elapsed_us":40,"counters":{"pages":12}}]}]}}"""
    Vector(
      "telemetry_sampling_policy" -> timed(plan)(policy.decide(12345L, 1000, failed = false, knownShape = true)),
      "telemetry_sampling_decide" -> timed(plan)(runtime.decide(shape.id, 1000, failed = false)),
      "telemetry_shape_extract" -> timed(plan)(QueryShapeExtractor.extract(bound)),
      "telemetry_cached_shape_id" -> timed(plan)(shape.id),
      "telemetry_event_construct" -> timed(plan)(QueryTelemetry(shape.id, 1, 1, 1, 10, 1000, true, false)),
      "telemetry_queue_offer_with_drain" -> timed(plan) { runtime.offer(event); if runtime.metrics.queueSize >= 60000 then runtime.drainNow() },
      "telemetry_profile_rows_scanned" -> timed(plan)(io.adb.gateway.ProfileRenderer.rowsScanned(profile))
    )

  /** The modes, the capture steps and the allocation figures, against the real engine. */
  private def engineCases(data: java.nio.file.Path, lib: java.nio.file.Path, plan: MeasurementPlan): Vector[(String, String)] =
    val (native, db) = openGateway(data, lib)
    try
      val savedPolicy = db.telemetry.policy
      // Sample so rarely that "sampled" is the skip path and "full" is the capture path.
      db.telemetry.policy = TelemetrySamplingPolicy(fastSampleEvery = 1 << 20, slowQueryNanos = Long.MaxValue)
      def inMode(mode: TelemetryMode): () => Any = () =>
        db.telemetry.mode = mode
        db.execute(GatewayPointSql)
      for _ <- 0 until 2000 do { db.telemetry.mode = TelemetryMode.Full; db.execute(GatewayPointSql) }
      val modes = Runner.interleaved(plan)(Seq(
        "gateway_point_telemetry_off" -> inMode(TelemetryMode.Off),
        "gateway_point_telemetry_skip" -> inMode(TelemetryMode.Sampled),
        "gateway_point_telemetry_capture" -> inMode(TelemetryMode.Full)
      ))
      db.telemetry.mode = TelemetryMode.Off
      val modeCases = modes.map((name, measured) => name -> timedCase(measured, 1.0))
      val breakdown = captureBreakdown(db, native, plan)
      val allocation = allocationCases(db)
      db.telemetry.policy = savedPolicy
      modeCases ++ breakdown ++ allocation
    finally native.close()

  /** The steps a captured query adds to the engine run, measured interleaved. */
  private def captureBreakdown(db: AdaptiveDatabase, native: io.adb.ffm.NativeDatabase, plan: MeasurementPlan): Vector[(String, String)] =
    val bound = new Binder(db.catalog).bind(new SqlParser().parse(GatewayPointSql)).asInstanceOf[BoundSelect]
    val plans = db.plan(bound)
    val json = PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch)
    val snapshot = Optional.empty[java.lang.Long]()
    def drain(query: NativeQuery): Int =
      var rows = 0
      var next = query.nextBatch()
      while next.isPresent do
        rows += next.get().rowCount()
        next = query.nextBatch()
      rows
    val execOnly = () =>
      val query = native.execute(json, snapshot)
      try drain(query).toLong finally query.close()
    val execProfile = () =>
      val query = native.execute(json, snapshot)
      try drain(query).toLong + query.profileJson().length finally query.close()
    val execProfileScan = () =>
      val query = native.execute(json, snapshot)
      try
        drain(query)
        io.adb.gateway.ProfileRenderer.rowsScanned(query.profileJson())
      finally query.close()
    Runner.interleaved(plan)(Seq(
      "capture_native_execute_only" -> execOnly,
      "capture_native_execute_profile" -> execProfile,
      "capture_native_execute_profile_scan" -> (() => execProfileScan().hashCode.toLong)
    )).map((name, measured) => name -> timedCase(measured, 1.0))

  /** Bytes the calling thread allocates per execution, per mode, and per single-row write with the
    * write counters off and on. Each figure is the smallest of six rounds: the JIT removes
    * allocations as it compiles, so a later round is lower for reasons unrelated to telemetry.
    */
  private def allocationCases(db: AdaptiveDatabase): Vector[(String, String)] =
    val threads = java.lang.management.ManagementFactory.getThreadMXBean.asInstanceOf[com.sun.management.ThreadMXBean]
    val runs = 2000
    def allocated(mode: String, sql: String): Double =
      db.execute(s"SET telemetry = $mode")
      for _ <- 0 until 1000 do db.execute(sql)
      val before = threads.getCurrentThreadAllocatedBytes
      var i = 0
      while i < runs do { db.execute(sql); i += 1 }
      (threads.getCurrentThreadAllocatedBytes - before).toDouble / runs
    val point = (0 until 6).map(_ => (allocated("off", GatewayPointSql), allocated("sampled", GatewayPointSql), allocated("full", GatewayPointSql)))
    val (off, skip, capture) = (point.map(_._1).min, point.map(_._2).min, point.map(_._3).min)
    db.execute("UPDATE account SET owner = 'probe' WHERE id = 1")
    val writeSql = "UPDATE account SET owner = 'probe' WHERE id = 1"
    val write = (0 until 6).map(_ => (allocated("off", writeSql), allocated("full", writeSql)))
    val (writeOff, writeOn) = (write.map(_._1).min, write.map(_._2).min)
    db.execute("SET telemetry = off")
    Vector(
      "alloc_point_off" -> valueCase(off, "unit" -> Json.str("bytes_per_execution")),
      "alloc_point_skip" -> valueCase(skip, "extra_vs_off" -> Json.num(skip - off)),
      "alloc_point_capture" -> valueCase(capture, "extra_vs_off" -> Json.num(capture - off)),
      "alloc_write_off" -> valueCase(writeOff, "unit" -> Json.str("bytes_per_execution")),
      "alloc_write_counted" -> valueCase(writeOn, "extra_vs_off" -> Json.num(writeOn - writeOff))
    )
