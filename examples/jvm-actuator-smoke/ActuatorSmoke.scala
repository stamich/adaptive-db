// End-to-end check of the Milestone 2.5.4 actuator through the real native library:
// nothing happens unless `auto_safe` is set; then the advisor creates an `adv_*` index for a
// wasteful scan, measures it (rows read per execution, exact engine counters) and keeps it; an index
// that does not reduce rows read is rolled back and not recreated during its cooldown; a user's index
// is never touched, even when idle; the budget rejects a second candidate with an audited reason; an
// index created before a restart is re-adopted; a person's drop is respected; `advisory` stops all
// action; and no query result changes in any mode.
//
// Run: examples/jvm-actuator-smoke/run.sh

import io.adb.actuator.{ActionState, AdaptiveConfig, AdaptiveMode, Budget, Policy}
import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import java.nio.file.{Files, Path}

/** Exits non-zero when any expectation fails. */
object ActuatorSmoke:
  /** Fails with `message` unless `condition` holds. */
  private def check(condition: Boolean, message: String): Unit =
    if !condition then
      System.err.println("FAILED: " + message)
      sys.exit(1)
    else println("ok: " + message)

  /** Small windows so a scenario takes a few ticks; the cooldowns are long, so they hold. */
  private val policy = Policy(minExecutions = 50L, cooldownNanos = 0L, rollbackCooldownNanos = 3_600_000_000_000L, verifyWindows = 3, idleWindows = 12)

  /** Names of the indexes that exist. */
  private def indexNames(db: AdaptiveDatabase): Set[String] = db.execute("SHOW INDEXES").rows.map(_.head.toString).toSet

  /** Events of the action log, oldest first. */
  private def events(db: AdaptiveDatabase): Vector[String] = db.showActionsLog().rows.map(_(1).toString)

  /** One tick of the controller: what the timer does every period, made deterministic. */
  private def tick(db: AdaptiveDatabase): Unit =
    db.telemetry.drainNow()
    db.controller.tick(db.physicalDesign())

  /** Runs `queries` statements built by `sql`, then one tick. */
  private def round(db: AdaptiveDatabase, queries: Int)(sql: Int => String): Unit =
    for n <- 1 to queries do db.execute(sql(n))
    tick(db)

  /** Rows the engine read per execution, summed over the captured shapes, from the telemetry counters. */
  private def rowsReadPerExecution(db: AdaptiveDatabase): Double =
    db.telemetry.drainNow()
    val stats = db.telemetry.allStats()
    stats.map(_.totalRowsRead).sum.toDouble / math.max(1L, stats.map(_.executions).sum)

  /** Runs the scenario against the library named by the first argument. */
  def main(args: Array[String]): Unit =
    val dir = Files.createTempDirectory("adb-actuator-smoke")
    val native = NativeDatabase.open(Path.of(args(0)), dir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native, adaptiveConfig = AdaptiveConfig(policy = policy))
      db.execute("CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL)")
      for i <- 1 to 2000 do db.execute(s"INSERT INTO orders (id, customer_id, amount) VALUES ($i, ${1 + i % 20}, ${i * 3})")
      // `flag` is the same for every row, so an index on it cannot reduce the rows a scan reads.
      db.execute("CREATE TABLE events (id BIGINT PRIMARY KEY, flag BIGINT NOT NULL, amount BIGINT NOT NULL)")
      for i <- 1 to 2000 do db.execute(s"INSERT INTO events (id, flag, amount) VALUES ($i, 1, ${i * 3})")
      val byCustomer = (n: Int) => s"SELECT id, amount FROM orders WHERE customer_id = ${1 + n % 20}"
      val byFlag = (n: Int) => s"SELECT id FROM events WHERE flag = 1 AND amount = ${3 * (1 + n % 2000)}"
      val customerResult = db.execute(byCustomer(3)).rows
      val flagResult = db.execute(byFlag(7)).rows

      // ---- off and advisory: nothing happens -------------------------------------------------
      check(db.adaptiveMode == AdaptiveMode.Off && !db.advisorRunning, "adaptive is off by default and no timer runs")
      db.execute("SET telemetry = full")
      for _ <- 1 to 6 do round(db, 100)(byCustomer)
      check(indexNames(db).isEmpty && db.showActions().rows.isEmpty, "while off, nothing is evaluated and nothing is created")
      db.controller.mode = AdaptiveMode.Advisory
      for _ <- 1 to 6 do round(db, 100)(byCustomer)
      check(indexNames(db).isEmpty && db.showActions().rows.isEmpty, "advisory recommends but never acts")
      check(db.showAdvice().rows.exists(_.head == "index"), "the advice is there")
      check(db.showActions().message.exists(_.contains("advisory")), "\\actions says that the mode is not auto_safe")

      // ---- auto_safe: create, measure, keep --------------------------------------------------
      val before = rowsReadPerExecution(db)
      db.controller.mode = AdaptiveMode.AutoSafe
      db.advisor.reset()                                  // forget the advisory evaluations: count three afresh
      round(db, 100)(byCustomer)                          // baseline of the model: not an interval yet
      round(db, 100)(byCustomer)
      round(db, 100)(byCustomer)
      check(indexNames(db).isEmpty, "two evaluations of the finding are not enough (hysteresis)")
      round(db, 100)(byCustomer)
      check(indexNames(db) == Set("adv_orders_customer_id"), "auto_safe creates the advisor index on the third evaluation, and nothing else")
      check(db.showActions().rows.head(3) == "applied", "the action is applied and being measured")
      for _ <- 1 to 4 do round(db, 100)(byCustomer)
      val kept = db.showActions().rows.head
      check(kept(3) == "kept", s"rows read per execution fell and the advisor keeps the index (${kept(5)})")
      check(events(db).containsSlice(Vector("applied", "kept")), "the action log tells the story")
      db.telemetry.drainNow()
      check(db.execute(byCustomer(3)).rows == customerResult, "the result is the same with the advisor's index")

      // ---- an index that does not help is rolled back and not recreated ----------------------
      // Both workloads run in every round, so the first index stays wanted (the idle rule is tested below).
      val mixed = (n: Int) => if n % 2 == 0 then byFlag(n) else byCustomer(n)
      for _ <- 1 to 3 do round(db, 100)(mixed)
      check(indexNames(db).contains("adv_events_flag"), "a second candidate is built")
      for _ <- 1 to 4 do round(db, 100)(mixed)
      check(!indexNames(db).contains("adv_events_flag"), "the index that did not reduce rows read is dropped again")
      check(events(db).contains("rolled_back"), "the rollback is logged")
      for _ <- 1 to 8 do round(db, 100)(mixed)
      check(!indexNames(db).contains("adv_events_flag"), "and it is not recreated during the cooldown")
      check(db.showActionsLog().rows.exists(r => r(1) == "rejected" && r(3).toString.startsWith("rollback_cooldown")), "the refusal is logged with its reason")
      check(indexNames(db) == Set("adv_orders_customer_id"), "the first index is untouched")
      check(db.execute(byFlag(7)).rows == flagResult, "the result is the same after the rollback")

      // ---- advisory stops further action; results are identical in every mode ----------------
      db.execute("SET adaptive = advisory")
      check(db.adaptiveMode == AdaptiveMode.Advisory, "SET adaptive = advisory")
      val applied = db.showActions().rows.size
      val logged = db.showActionsLog().rows.size
      for _ <- 1 to 4 do round(db, 100)(mixed)
      check(db.showActions().rows.size == applied && db.showActionsLog().rows.size == logged, "advisory stops new actions at once")
      check(db.execute(byCustomer(3)).rows == customerResult && db.execute(byFlag(7)).rows == flagResult, "results are identical in every mode")
      db.execute("SET adaptive = off")
      check(!db.advisorRunning, "SET adaptive = off stops the timer")

      // ---- a user's index is never touched, even when idle; an idle advisor index is dropped --
      db.execute("CREATE INDEX ix_user_amount ON orders (amount)")
      db.controller.mode = AdaptiveMode.AutoSafe
      for _ <- 1 to 14 do round(db, 100)(n => s"SELECT id FROM events WHERE id = ${1 + n % 2000}")
      check(indexNames(db).contains("ix_user_amount"), "the user's index survives the idle windows")
      check(!indexNames(db).contains("adv_orders_customer_id"), "the advisor's own index, wanted by nobody for idleWindows windows, is dropped")
      check(db.showActionsLog().rows.exists(r => r(1) == "rolled_back" && r(3).toString.contains("idle")), "the idle drop is logged with its reason")

      // ---- a person's drop is respected -----------------------------------------------------
      db.execute("CREATE TABLE visits (id BIGINT PRIMARY KEY, k BIGINT NOT NULL, v BIGINT NOT NULL)")
      for i <- 1 to 2000 do db.execute(s"INSERT INTO visits (id, k, v) VALUES ($i, ${1 + i % 25}, $i)")
      val byKey = (n: Int) => s"SELECT id FROM visits WHERE k = ${1 + n % 25}"
      for _ <- 1 to 3 do round(db, 100)(byKey)
      check(indexNames(db).contains("adv_visits_k"), "a new candidate is built")
      for _ <- 1 to 4 do round(db, 100)(byKey)
      db.execute("DROP INDEX adv_visits_k")
      for _ <- 1 to 6 do round(db, 100)(byKey)
      check(!indexNames(db).contains("adv_visits_k"), "an advisor index dropped by a person is not recreated")
      check(events(db).contains("externally_dropped"), "and the drop is logged")

      // ---- the budget ----------------------------------------------------------------------
      val tight = AdaptiveDatabase.open(native, adaptiveConfig = AdaptiveConfig(policy = policy, budget = Budget(maxAdvisorIndexes = 1)))
      tight.telemetry.mode = io.adb.telemetry.TelemetryMode.Full
      tight.controller.mode = AdaptiveMode.AutoSafe
      tight.execute("CREATE TABLE seen (id BIGINT PRIMARY KEY, a BIGINT NOT NULL, b BIGINT NOT NULL)")
      for i <- 1 to 1000 do tight.execute(s"INSERT INTO seen (id, a, b) VALUES ($i, ${i % 50}, ${i % 70})")
      for _ <- 1 to 6 do
        for n <- 1 to 100 do { tight.execute(s"SELECT id FROM seen WHERE a = ${n % 50}"); tight.execute(s"SELECT id FROM seen WHERE b = ${n % 70}") }
        tick(tight)
      val advisorIndexes = indexNames(tight).filter(_.startsWith("adv_"))
      check(advisorIndexes.size == 1, s"a budget of one advisor index creates one ($advisorIndexes)")
      check(tight.showActionsLog().rows.exists(r => r(1) == "rejected" && r(3).toString.startsWith("too_many_indexes")),
        "the second candidate is rejected with an audited reason")

      // ---- re-adoption after a restart ------------------------------------------------------
      tight.execute("SET adaptive = off")
      val restarted = AdaptiveDatabase.open(native, adaptiveConfig = AdaptiveConfig(policy = policy))
      restarted.telemetry.mode = io.adb.telemetry.TelemetryMode.Full
      restarted.controller.mode = AdaptiveMode.AutoSafe
      restarted.advisor.evaluate(restarted.physicalDesign())
      tick(restarted)
      check(restarted.controller.registry.mayDrop(advisorIndexes.head), "an adv_* index created before the restart is adopted")
      check(!restarted.controller.registry.mayDrop("ix_user_amount"), "a user's index is not")
      check(restarted.showActionsLog().rows.exists(_(1) == "adopted"), "the adoption is logged")

      // ---- the real timer and the telemetry switch ------------------------------------------
      val timed = AdaptiveDatabase.open(native, adaptiveConfig = AdaptiveConfig(policy = policy))
      check(timed.telemetry.mode == io.adb.telemetry.TelemetryMode.Off, "telemetry starts off")
      timed.execute("SET adaptive = auto_safe")
      check(timed.telemetry.mode == io.adb.telemetry.TelemetryMode.Full && timed.advisorRunning, "auto_safe switches telemetry to full and starts the timer")
      timed.execute("SET adaptive = off")
      check(timed.telemetry.mode == io.adb.telemetry.TelemetryMode.Off && !timed.advisorRunning, "leaving auto_safe restores telemetry and stops the timer")
      println("actuator smoke test passed")
    finally native.close()
