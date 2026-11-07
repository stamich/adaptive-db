package io.adb.benchmark

import io.adb.bench.*
import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import io.adb.logical.{Binder, BoundCreateTable}
import io.adb.sql.SqlParser

import java.nio.file.{Files, Path}

/** Settings shared by the JVM benchmark families.
  *
  * @param dataDir   directory the gateway database lives in (created if missing)
  * @param nativeLib native library; families that need the engine are skipped without it
  * @param out       report path
  */
final case class FamilySettings(dataDir: Option[Path], nativeLib: Option[Path], out: Option[Path])

/** Helpers every family uses: cases, skipped cases, the benchmark database and the report. */
object FamilyCommon:
  /** DDL of the benchmark schema. */
  val Ddl: Vector[String] = Vector(
    "CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);",
    "CREATE TABLE transfer (id BIGINT PRIMARY KEY, account_id BIGINT NOT NULL, amount BIGINT NOT NULL);"
  )

  /** Point lookup used since 2.0. */
  val PointSql = "SELECT id, owner, balance FROM account WHERE id = 42 LIMIT 10;"

  /** The point lookup the gateway cases run (a row that exists in the benchmark data). */
  val GatewayPointSql = "SELECT * FROM account WHERE id = 500;"

  /** Relational query of 2.1: join, aggregate, ordered limit. */
  val RelationalSql: String =
    "SELECT a.owner, COUNT(*) AS n, SUM(t.amount) AS total FROM account a JOIN transfer t ON t.account_id = a.id " +
      "WHERE t.amount > 0 GROUP BY a.owner ORDER BY total DESC LIMIT 10;"

  /** The CPU-bound plan chosen by the environment (`ADB_BENCH_QUICK`, `ADB_BENCH_MIN_SAMPLE_MS`). */
  def cpuPlan: MeasurementPlan = MeasurementPlan.fromEnv(durable = false)

  /** A case for an operation measured per invocation: the legacy-style keys plus `ns_op`, `ops_s`
    * and the harness's `timing` and `jit`.
    */
  def timed(plan: MeasurementPlan)(body: => Any): String =
    val measured = Runner.measure(plan)(body)
    timedCase(measured, units = 1.0)

  /** The JSON of a measured case of `units` units of work per invocation. */
  def timedCase[T](measured: Measured[T], units: Double, extra: Seq[(String, String)] = Seq.empty): String =
    val stats = SampleStats.fromMeasured(measured)
    ReportJson.caseJson(
      stats,
      Some(measured.jit),
      Seq(
        "ns_op" -> Json.num(stats.medianNs / units),
        "ops_s" -> Json.num(ReportJson.ratePerSecond(stats, units))
      ) ++ extra
    )

  /** A case that was not run, with the reason. The validator accepts it; the gates report it. */
  def skipped(reason: String): String = Json.obj("skipped_reason" -> Json.str(reason))

  /** A case that is a number with no timing (an allocation, a count): `value` and `extra`. */
  def valueCase(value: Double, extra: (String, String)*): String =
    Json.obj((("value" -> Json.num(value)) +: extra)*)

  /** Opens the benchmark database (account: 1 000 rows, transfer: 10 000 rows, loaded once) in
    * `dataDir`; the caller closes the returned native handle.
    */
  def openGateway(dataDir: Path, nativeLib: Path): (NativeDatabase, AdaptiveDatabase) =
    Files.createDirectories(dataDir)
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    val db = AdaptiveDatabase.open(native, Some(dataDir.resolve("catalog.properties")))
    if db.catalog.entity("account").isEmpty then
      Ddl.foreach(db.execute)
      for i <- 1 to 1000 do db.execute(s"INSERT INTO account VALUES ($i, ${i * 10L}, 'owner-${i % 50}');")
      for i <- 1 to 10000 do db.execute(s"INSERT INTO transfer VALUES ($i, ${1 + i % 1000}, ${i % 97});")
    (native, db)

  /** Registers the benchmark tables in an in-memory catalog (planner cases need no engine). */
  def createTables(catalog: io.adb.catalog.Catalog): Unit =
    val parser = new SqlParser
    val binder = new Binder(catalog)
    Ddl.foreach { ddl =>
      binder.bind(parser.parse(ddl)) match
        case create: BoundCreateTable => catalog.createEntity(create.name, create.fields, create.primaryKey)
        case _ => throw new IllegalStateException("expected CREATE TABLE")
    }

  /** Builds the report of `family` with `cases` and writes it to `settings.out`; returns the text. */
  def finish(family: String, plan: MeasurementPlan, settings: FamilySettings, extra: Seq[(String, String)], cases: Seq[(String, String)]): String =
    val report = new Report(family, "2.5.4.1", plan, Environment.capture())
    extra.foreach((key, value) => report.set(key, value))
    report.set("cases", Json.obj(cases*))
    val text = report.finish()
    settings.out.foreach(path => Files.writeString(path, text + "\n", java.nio.charset.StandardCharsets.UTF_8))
    text
