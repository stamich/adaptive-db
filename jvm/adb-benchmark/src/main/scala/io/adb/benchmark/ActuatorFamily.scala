package io.adb.benchmark

import io.adb.actuator.AdaptiveMode
import io.adb.bench.*
import io.adb.benchmark.FamilyCommon.*
import io.adb.gateway.AdaptiveDatabase

/** The `actuator` family (Milestone 2.5.4.1): what the 2.5.4 actuator costs.
  *
  * * **Tick**: one evaluation of the controller (drain telemetry, evaluate the advisor, decide), in
  *   advisory mode and in `auto_safe` with nothing to act on. It runs off the query path, every few
  *   seconds, so this is the cost of a tick and not a per-query cost.
  * * **CREATE INDEX stall**: `CREATE INDEX` blocks the database for the whole build (about 30 us per
  *   row), which is why the actuator builds only on tables of up to 50 000 rows. The family builds an
  *   index on tables of 5 000, 20 000 and 50 000 rows (500, 2 000 and 5 000 with the quick plan) and
  *   reports the stall and its microseconds per row, so the limit can be checked against the
  *   machine. Dropping the index before each build is not timed.
  *
  * The counts the actuator produces (applied, kept, rolled back) are not timings and stay in the
  * workload report, where `check-benchmarks.py` holds them exactly.
  */
object ActuatorFamily:
  /** Table sizes of the stall cases: release and quick. */
  private val StallRows = Vector(5_000, 20_000, 50_000)

  /** Quick table sizes, so a smoke run finishes in seconds. */
  private val StallRowsQuick = Vector(500, 2_000, 5_000)

  /** Runs the family and returns the report JSON. */
  def run(settings: FamilySettings): String =
    val cpu = cpuPlan
    val durable = MeasurementPlan.fromEnv(durable = true)
    val sizes = if cpu.isQuick then StallRowsQuick else StallRows
    val cases = (settings.dataDir, settings.nativeLib) match
      case (Some(data), Some(lib)) => engineCases(data, lib, cpu, durable, sizes)
      case _ =>
        (Vector("controller_tick_advisory", "controller_tick_auto_safe_idle") ++ sizes.map(stallName))
          .map(_ -> skipped("no --data and --native-lib given"))
    finish("actuator", cpu, settings, Seq("plan_durable" -> durable.json, "stall_rows" -> Json.arr(sizes.map(n => Json.num(n.toLong)))), cases)

  /** Name of the stall case for a table of `rows` rows. */
  private def stallName(rows: Int): String = s"create_index_stall_${rows}_rows"

  /** The cases that need the engine. */
  private def engineCases(
      data: java.nio.file.Path,
      lib: java.nio.file.Path,
      cpu: MeasurementPlan,
      durable: MeasurementPlan,
      sizes: Vector[Int]
  ): Vector[(String, String)] =
    val (native, db) = openGateway(data, lib)
    try
      Vector(
        "controller_tick_advisory" -> tickCase(db, AdaptiveMode.Advisory, cpu),
        "controller_tick_auto_safe_idle" -> tickCase(db, AdaptiveMode.AutoSafe, cpu)
      ) ++ sizes.map(rows => stallName(rows) -> stallCase(db, rows, durable))
    finally native.close()

  /** One controller tick in `mode`, with telemetry on and no workload to act on. */
  private def tickCase(db: AdaptiveDatabase, mode: AdaptiveMode, plan: MeasurementPlan): String =
    db.execute("SET telemetry = full")
    db.controller.mode = mode
    try
      db.advisor.reset()
      timed(plan) {
        db.telemetry.drainNow()
        db.controller.tick(db.physicalDesign())
      }
    finally
      db.controller.mode = AdaptiveMode.Off
      db.execute("SET telemetry = off")

  /** The stall of `CREATE INDEX` on a table of `rows` rows: time, and microseconds per row. */
  private def stallCase(db: AdaptiveDatabase, rows: Int, plan: MeasurementPlan): String =
    val table = s"stall_$rows"
    if db.catalog.entity(table).isEmpty then
      db.execute(s"CREATE TABLE $table (id BIGINT PRIMARY KEY, k BIGINT NOT NULL, v BIGINT NOT NULL)")
      for i <- 1 to rows do db.execute(s"INSERT INTO $table VALUES ($i, ${i % 100}, ${i * 3L})")
    val index = s"stall_k_$rows"
    val measured = Runner.measureWithSetup(plan) {
      try db.execute(s"DROP INDEX $index")
      catch case _: Exception => () // absent on the first build
    } { _ => db.execute(s"CREATE INDEX $index ON $table (k)").rows.size }
    val stats = SampleStats.fromMeasured(measured)
    ReportJson.caseJson(
      stats,
      Some(measured.jit),
      Seq(
        "table_rows" -> Json.num(rows.toLong),
        "stall_ms" -> Json.num(stats.medianNs / 1e6),
        "us_per_row" -> Json.num(stats.medianNs / 1e3 / rows)
      )
    )
