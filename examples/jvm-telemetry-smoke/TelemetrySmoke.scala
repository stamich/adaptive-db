// End-to-end check of the Milestone 2.5.2 telemetry hot path through the real native library:
// off by default and silent, skipped queries leave no event, captured queries are aggregated
// per literal-free shape, a full queue drops events without failing queries, failures are
// captured, and telemetry never changes a result.
//
// Run: examples/jvm-telemetry-smoke/run.sh

import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import java.nio.file.{Files, Path}

/** Exits non-zero when any expectation fails. */
object TelemetrySmoke:
  /** Fails with `message` unless `condition` holds. */
  private def check(condition: Boolean, message: String): Unit =
    if !condition then
      System.err.println("FAILED: " + message)
      sys.exit(1)
    else println("ok: " + message)

  /** Runs the scenario against the library named by the first argument. */
  def main(args: Array[String]): Unit =
    val dir = Files.createTempDirectory("adb-telemetry-smoke")
    val native = NativeDatabase.open(Path.of(args(0)), dir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native)
      val t = db.telemetry
      db.execute("CREATE TABLE customer (id BIGINT PRIMARY KEY, name STRING NOT NULL)")
      db.execute("CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL)")
      for i <- 1 to 20 do db.execute(s"INSERT INTO customer (id, name) VALUES ($i, 'c$i')")
      for i <- 1 to 200 do db.execute(s"INSERT INTO orders (id, customer_id, amount) VALUES ($i, ${1 + i % 20}, ${i * 3})")
      val point = (n: Int) => s"SELECT id, amount FROM orders WHERE customer_id = $n"
      val join = "SELECT c.name, o.amount FROM customer c JOIN orders o ON c.id = o.customer_id WHERE o.amount > 10 ORDER BY o.amount DESC LIMIT 5"

      check(!t.enabled && t.metrics.captured == 0, "telemetry is off by default")
      val reference = db.execute(point(3)).rows
      check(t.metrics.skipped == 0 && t.metrics.captured == 0 && t.metrics.accepted == 0, "while off, queries touch no counter")

      check(db.execute("SET telemetry = full").message.exists(_.contains("full")), "SET telemetry = full")
      for n <- 1 to 5 do db.execute(point(n))
      db.execute(join); db.execute(join)
      check(t.metrics.captured == 7 && t.metrics.accepted == 7, "full mode captures every query")
      check(db.execute(point(3)).rows == reference, "telemetry does not change a result")
      t.drainNow()
      val stats = t.allStats()
      check(stats.size == 2, "five point queries with different literals are one shape, the join another")
      val pointStats = stats.find(_.executions >= 5).get
      check(pointStats.totalRowsRead >= pointStats.totalActualRows && pointStats.maxNanos > 0, "events carry rows returned, rows read and time")
      check(stats.forall(_.totalRowsRead > 0), "every captured shape reports the rows its scans read (the engine counters of C ABI 9)")
      check(stats.exists(s => s.totalRowsRead > s.totalActualRows * 2), "scan amplification is visible: a join reads more rows than it returns")
      val joinStats = stats.find(_.executions == 2).get
      check(joinStats.totalRowsRead == 2 * 220L, "rows read come from the engine counters: both scans of the join, 20 + 200 rows, per execution")
      check(stats.exists(_.cacheHits >= 1), "a repeated statement is recorded as a plan-cache hit")
      check(t.registry.size == 2, "each shape is registered once")

      db.execute("SET telemetry = sampled")
      val before = t.metrics
      for n <- 1 to 300 do db.execute(point(1 + n % 7))
      val after = t.metrics
      check(after.skipped - before.skipped > 200, "sampled mode skips most known, fast queries")
      check(after.captured - before.captured < 100, "and captures only a few")

      db.execute("SET telemetry = off")
      val frozen = t.metrics
      db.execute(point(2))
      check(t.metrics.skipped == frozen.skipped && t.metrics.captured == frozen.captured, "switching off stops all telemetry work")

      val tiny = new io.adb.telemetry.TelemetryRuntime(capacity = 2, startWorker = false)
      tiny.mode = io.adb.telemetry.TelemetryMode.Full
      val ev = io.adb.telemetry.QueryTelemetry(1, 1, 1, 1, 1, 1, false, false)
      check(tiny.offer(ev) && tiny.offer(ev) && !tiny.offer(ev) && tiny.metrics.dropped == 1, "a full queue drops the event and returns at once")

      db.execute("SET telemetry = full")
      check(scala.util.Try(db.execute("SELECT id FROM orders WHERE nope = 1")).isFailure, "a statement that fails to bind still fails")
      check(db.showTelemetry().rows.exists(_.head == "mode") && db.showTelemetryShapes().rows.nonEmpty, "\\telemetry and \\telemetry shapes list the counters and the shapes")
      println("telemetry smoke passed")
    finally native.close()
