// End-to-end check of the Milestone 2.5.3 workload advisor through the real native library:
// it sees only what telemetry captured, it recommends an index for a wasteful scan only after the
// advice held for three evaluations, it never acts on its own, its advice disappears once the
// index exists, the audit log tells the story, and none of this changes a query result.
//
// Run: examples/jvm-advisor-smoke/run.sh

import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import java.nio.file.{Files, Path}

/** Exits non-zero when any expectation fails. */
object AdvisorSmoke:
  /** Fails with `message` unless `condition` holds. */
  private def check(condition: Boolean, message: String): Unit =
    if !condition then
      System.err.println("FAILED: " + message)
      sys.exit(1)
    else println("ok: " + message)

  /** The `sql` column of the advice rows whose kind is `kind`. */
  private def sqlOf(advice: QueryResult, kind: String): Vector[String] =
    advice.rows.filter(_.head == kind).map(_(2).toString)

  /** Runs the scenario against the library named by the first argument. */
  def main(args: Array[String]): Unit =
    val dir = Files.createTempDirectory("adb-advisor-smoke")
    val native = NativeDatabase.open(Path.of(args(0)), dir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native)
      db.execute("CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL)")
      for i <- 1 to 2000 do db.execute(s"INSERT INTO orders (id, customer_id, amount) VALUES ($i, ${1 + i % 20}, ${i * 3})")
      val point = (n: Int) => s"SELECT id, amount FROM orders WHERE customer_id = $n"
      val reference = db.execute(point(3)).rows

      check(!db.telemetry.enabled && !db.advisorRunning, "telemetry and the advisor are off by default")
      check(db.showAdvice().message.exists(_.contains("telemetry is off")), "with telemetry off the advisor says it sees nothing")
      db.execute("SET telemetry = full")

      // One evaluation per round of 100 queries. The evaluation above was the baseline, so each
      // round is one window; the advice must hold for three of them.
      def round(): QueryResult =
        for n <- 1 to 100 do db.execute(point(1 + n % 20))
        db.showAdvice()
      check(sqlOf(round(), "index").isEmpty, "a finding seen once is not shown yet (hysteresis)")
      check(sqlOf(round(), "index").isEmpty, "a finding seen twice is not shown yet")
      val advice = round()
      check(
        sqlOf(advice, "index") == Vector("CREATE INDEX adv_orders_customer_id ON orders (customer_id)"),
        "the third evaluation recommends an index on the filtered column"
      )
      check(advice.rows.exists(_.head == "hot-query"), "the dominant query is reported as hot")
      check(db.execute("SHOW INDEXES").rows.isEmpty, "the advisor only recommends: no index was created")
      check(db.execute(point(3)).rows == reference, "advising does not change a result")

      // Carry the advice out by hand; it must leave the report as applied.
      db.execute(sqlOf(advice, "index").head)
      check(sqlOf(round(), "index").isEmpty, "once the index exists the advice is gone")
      val log = db.showAdviceLog().rows.map(_(1).toString)
      check(log.contains("confirmed") && log.contains("applied"), "the audit log records that the advice was confirmed and then applied")
      check(db.execute(point(3)).rows == reference, "the result is the same with the index")

      // The timer is optional and switchable.
      check(db.execute("SET advisor = on").message.exists(_.contains("on")) && db.advisorRunning, "SET advisor = on starts the timer")
      check(db.execute("SET advisor = off").message.exists(_.contains("off")) && !db.advisorRunning, "SET advisor = off stops it")
      check(db.showAdviceLog().rows.nonEmpty, "the audit log survives switching the timer off")
      println("advisor smoke test passed")
    finally native.close()
