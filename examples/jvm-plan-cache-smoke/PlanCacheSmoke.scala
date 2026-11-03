// End-to-end check of the Milestone 2.5.1 plan cache through the real native library: hits after
// the second execution, invalidation by DDL / ANALYZE / index creation, separate entries per
// optimizer mode, no caching of AS OF, DML not invalidating, a stale token still refused on a hit,
// DDL from another handle, and the on/off switch.
//
// Run: examples/jvm-plan-cache-smoke/run.sh

import io.adb.consistency.*
import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import java.nio.file.{Files, Path}

/** Exits non-zero when any expectation fails. */
object PlanCacheSmoke:
  /** Fails with `message` unless `condition` holds. */
  private def check(condition: Boolean, message: String): Unit =
    if !condition then
      System.err.println("FAILED: " + message)
      sys.exit(1)
    else println("ok: " + message)

  /** Runs the scenario against the library named by the first argument. */
  def main(args: Array[String]): Unit =
    val dir = Files.createTempDirectory("adb-plan-cache-smoke")
    val native = NativeDatabase.open(Path.of(args(0)), dir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native)
      db.execute("CREATE TABLE account (id BIGINT PRIMARY KEY, region BIGINT NOT NULL, balance BIGINT NOT NULL)")
      for i <- 1 to 50 do db.execute(s"INSERT INTO account (id, region, balance) VALUES ($i, ${i % 5}, ${i * 10})")
      val q = "SELECT id, balance FROM account WHERE region = 3 ORDER BY id"
      def rows(sql: String = q) = db.execute(sql).rows
      def stats = db.planCacheStats

      val first = rows()
      check(stats.hits == 0 && stats.deferred == 1, "the first execution is planned and only remembered")
      rows()
      check(stats.inserts == 1 && stats.hits == 0, "the second execution stores the plan")
      val third = rows()
      check(stats.hits == 1 && third == first, "the third execution is a hit with the same rows")
      check(rows("select id, balance FROM account   WHERE region = 3 -- again\n ORDER BY id").size == first.size, "layout and comments do not change the answer")

      db.execute("INSERT INTO account (id, region, balance) VALUES (100, 3, 5)")
      val hitsBefore = stats.hits
      check(rows().size == first.size + 1 && stats.hits == hitsBefore + 1, "DML does not invalidate: a hit that sees the new row")

      db.execute("ALTER TABLE account ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR'")
      val staleBefore = stats.stale
      check(rows().size == first.size + 1 && stats.stale == staleBefore + 1, "ALTER TABLE makes the entry stale and the query is planned again")

      rows(); rows()
      db.execute("ANALYZE account")
      val s2 = stats.stale
      rows()
      check(stats.stale == s2 + 1, "ANALYZE makes the entry stale")

      rows(); rows()
      db.execute("CREATE INDEX by_region ON account (region)")
      val s3 = stats.stale
      rows()
      check(stats.stale == s3 + 1, "CREATE INDEX makes the entry stale")
      val explain = db.execute("EXPLAIN " + q).rows.map(_.head.toString).mkString("\n")
      check(explain.contains("index by_region") && explain.contains("Plan cache: cacheable"), "the new plan uses the index and EXPLAIN says it is cacheable")
      rows(); rows()
      val h4 = stats.hits
      rows()
      check(stats.hits == h4 + 1, "the index plan is cached too")

      db.execute("SET optimizer = rule")
      val m = stats.misses
      rows()
      check(stats.misses == m + 1 && rows().size == first.size + 1, "another optimizer mode has its own entry")
      db.execute("SET optimizer = cost")
      val h5 = stats.hits
      rows()
      check(stats.hits == h5 + 1, "switching back finds the cost-mode entry again")

      val asOf = "SELECT id FROM account AS OF VERSION 1 WHERE id = 1"
      val ins = stats.inserts
      scala.util.Try { db.execute(asOf); db.execute(asOf); db.execute(asOf) }
      check(stats.inserts == ins, "AS OF statements are never stored")

      val ahead = db.session()
      ahead.observe(SessionConsistencyToken(100000))
      check(scala.util.Try(ahead.execute(q)).isFailure, "a hit does not bypass the refusal of a token beyond the head")

      val other = AdaptiveDatabase.open(native)
      rows(); rows()
      val s6 = stats.stale
      other.execute("CREATE TABLE note (id BIGINT PRIMARY KEY, body STRING NOT NULL)")
      check(rows().nonEmpty && stats.stale == s6 + 1, "DDL through another handle invalidates this handle's entry")

      check(db.execute("SET plan_cache = off").message.exists(_.contains("off")), "SET plan_cache = off")
      val h6 = stats.hits
      rows(); rows(); rows()
      check(stats.hits == h6, "a disabled cache is not consulted")
      db.execute("SET plan_cache = on")
      check(db.showPlanCache().rows.exists(_.head == "hits") && db.clearPlanCache() >= 1, "\\cache lists counters and the cache can be cleared")
      println("plan cache smoke passed")
    finally native.close()
