// End-to-end check of Milestone 2.5.0 through the real native library: sessions, tokens,
// SET SESSION CONSISTENCY, EXPLAIN's read context, projections and the change log.
//
// Run: examples/jvm-consistency-smoke/run.sh

import io.adb.consistency.*
import io.adb.ffm.NativeDatabase
import io.adb.gateway.AdaptiveDatabase
import java.nio.file.{Files, Path}

/** Exits non-zero when any expectation fails. */
object ConsistencySmoke:
  /** Fails with `message` unless `condition` holds. */
  private def check(condition: Boolean, message: String): Unit =
    if !condition then
      System.err.println("FAILED: " + message)
      sys.exit(1)
    else println("ok: " + message)

  /** Runs the scenario against the library named by the first argument. */
  def main(args: Array[String]): Unit =
    val dir = Files.createTempDirectory("adb-consistency-smoke")
    val native = NativeDatabase.open(Path.of(args(0)), dir.resolve("rust"))
    try
      val db = AdaptiveDatabase.open(native)
      val a = db.session()
      val b = db.session()
      check(a.token == SessionConsistencyToken.Initial, "a new session has the initial token")
      check(a.consistency == ReadConsistency.ReadYourWrites, "sessions read their own writes by default")

      val created = a.execute("CREATE TABLE account (id BIGINT PRIMARY KEY, region BIGINT NOT NULL, balance BIGINT NOT NULL)")
      check(created.commitSeq.contains(1L) && a.token.commitSeq == 1, "DDL moves the token to its commit (1)")
      check(b.token.commitSeq == 0, "another session's token does not move")

      val insert = a.execute("INSERT INTO account (id, region, balance) VALUES (1, 7, 100)")
      check(insert.commitSeq.contains(2L) && a.token.commitSeq == 2, "an INSERT moves the token to its commit (2)")
      val index = a.execute("CREATE INDEX by_region ON account (region)")
      check(index.commitSeq.exists(_ >= 3) && a.token.commitSeq >= 3, "CREATE INDEX is a commit in the same order")
      val update = a.execute("UPDATE account SET balance = 150 WHERE id = 1")
      check(update.commitSeq.exists(_ > index.commitSeq.get), "DML after DDL has a later number")

      val rows = a.execute("SELECT id, balance FROM account WHERE region = 7").rows
      check(rows == Vector(Vector(1L, 150L)), "the session reads its own write through the index")

      check(a.execute("SET SESSION CONSISTENCY STRONG").message.contains("SET SESSION CONSISTENCY STRONG"), "SET SESSION CONSISTENCY STRONG")
      check(a.consistency == ReadConsistency.StrongLatest, "the level changed")
      check(b.consistency == ReadConsistency.ReadYourWrites, "the other session keeps its level")
      a.execute("SET SESSION CONSISTENCY BOUNDED 250 MS")
      check(a.consistency == ReadConsistency.BoundedStaleness(StalenessBound.Millis(250)), "bounded form")
      a.execute("SET SESSION CONSISTENCY EVENTUAL")

      val explain = a.execute("EXPLAIN SELECT id FROM account WHERE region = 7").rows.map(_.head.toString).mkString("\n")
      check(explain.contains("Read context:") && explain.contains("consistency=EVENTUAL") && explain.contains("index by_region"), "EXPLAIN shows the read context")

      val projections = db.showProjections().rows.map(_(1))
      check(projections.contains("__current_row") && projections.contains("index:by_region"), "projections are reported")
      val changes = db.showChanges(0, 20)
      check(changes.rows.size >= 4 && changes.rows.head(3) == "schema", "the change log lists the commits in order")

      val ahead = db.session()
      ahead.observe(SessionConsistencyToken(1000))
      check(scala.util.Try(ahead.execute("SELECT id FROM account")).isFailure, "a token beyond the head is refused")
      ahead.consistency = ReadConsistency.Eventual
      check(scala.util.Try(ahead.execute("SELECT id FROM account")).isSuccess, "eventual reads ignore the token")
      println("consistency smoke passed")
    finally native.close()
