package io.adb.cli

import io.adb.catalog.FileCatalog
import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import java.nio.file.{Files, Path}
import scala.jdk.CollectionConverters.*

/**
 * Runs the Milestone 2.0.1 feature tour.
 *
 * The demo deliberately executes on one current 2.0.1 engine while presenting features in the
 * order in which they entered the project: WAL/recovery, persistent current storage, persistent
 * history, native batch execution, and finally the Scala SQL/control plane.
 */
object DemoMain:
  private val CommitTs = raw"commitTs=(\d+)".r

  /** Executes the complete chronological feature tour. */
  def main(args: Array[String]): Unit = run()

  /** Opens the configured demo database, executes all phases, and verifies restart persistence. */
  def run(): Unit =
    val dataDir = Path.of(sys.env.getOrElse("ADB_DATA", "./adb-demo-data")).toAbsolutePath.normalize()
    val nativeLib = Path.of(
      sys.env.getOrElse("ADB_NATIVE_LIBRARY", sys.props.getOrElse("adb.native.library", "../target/release/libadb_ffi.so"))
    ).toAbsolutePath.normalize()

    requireEmptyDemoDirectory(dataDir)
    Files.createDirectories(dataDir)

    banner("Adaptive DB 2.0.1 — chronological feature tour")
    println(s"data directory : $dataDir")
    println(s"native library : $nativeLib")

    var account2InitialVersion = 0L

    withDatabase(dataDir, nativeLib) { db =>
      phase("Bootstrap", "2.0.1 convenience layer")
      execute(db, "CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);")

      phase("1.0.1", "transactions + WAL durability")
      val first = execute(db, "INSERT INTO account VALUES (1, 100, 'Alice');")
      commitTs(first)
      account2InitialVersion = commitTs(execute(db, "INSERT INTO account VALUES (2, 200, 'Bob');"))
      execute(db, "SELECT * FROM account;")
      println("Feature shown: committed mutations are durable before visibility through the WAL path.")
    }

    phase("1.0.1", "restart / recovery")
    println("The native database was closed and is now reopened from the same directory.")
    withDatabase(dataDir, nativeLib) { db =>
      execute(db, "SELECT * FROM account;")
      println("Feature shown: committed rows survive reopening and recovery.")

      phase("1.5.1", "persistent Current Store + primary B+Tree")
      execute(db, "EXPLAIN SELECT * FROM account WHERE id = 2;")
      execute(db, "SELECT * FROM account WHERE id = 2;")
      println("Feature shown: primary-key equality can be optimized to a point lookup over persistent current state.")

      phase("1.6.1", "persistent Version Store + historical MVCC")
      execute(db, "UPDATE account SET balance = 250 WHERE id = 2;")
      execute(db, "SELECT * FROM account WHERE id = 2;")
      execute(db, s"SELECT * FROM account AS OF VERSION $account2InitialVersion WHERE id = 2;")
      println(s"Feature shown: AS OF VERSION $account2InitialVersion returns the pre-update version.")

      phase("1.7.1", "physical execution + RecordBatch + native FFI")
      execute(db, "EXPLAIN ANALYZE SELECT id, owner, balance FROM account WHERE balance >= 100 LIMIT 10;")
      execute(db, "SELECT id, owner, balance FROM account WHERE balance >= 100 LIMIT 10;")
      println("Feature shown: physical plans cross the Java FFM boundary and results return as bounded native record batches.")

      phase("2.0.1", "SQL + catalog + binder + optimizer + physical planner + gateway")
      execute(db, "INSERT INTO account VALUES (3, 300, 'Carol');")
      execute(db, "UPDATE account SET balance = 275 WHERE id = 2;")
      execute(db, "DELETE FROM account WHERE id = 1;")
      execute(db, "SELECT id, owner, balance FROM account WHERE balance > 200 LIMIT 10;")
      execute(db, "EXPLAIN SELECT * FROM account WHERE id = 3;")
      println("Feature shown: end-to-end SQL is parsed and bound in Scala, planned/optimized, then executed by the Rust engine.")
    }

    banner("Demo completed successfully")
    println("Re-run demo/run-demo.sh to recreate the database from scratch.")

  /** Opens one native database and matching persistent JVM catalog for a demo phase. */
  private def withDatabase(dataDir: Path, nativeLib: Path)(body: AdaptiveDatabase => Unit): Unit =
    val catalog = new FileCatalog(dataDir.resolve("catalog.properties"))
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    try body(new AdaptiveDatabase(catalog, native))
    finally native.close()

  /** Executes one SQL statement and renders the resulting message or tabular rows. */
  private def execute(db: AdaptiveDatabase, sql: String): QueryResult =
    println()
    println(s"adb> $sql")
    val result = db.execute(sql)
    result.message.foreach(println)
    if result.columns.nonEmpty then
      println(result.columns.mkString(" | "))
      result.rows.foreach(row => println(row.map(v => Option(v).getOrElse("NULL")).mkString(" | ")))
    result

  /** Extracts the native commit timestamp from a successful mutation message. */
  private def commitTs(result: QueryResult): Long =
    result.message
      .flatMap(message => CommitTs.findFirstMatchIn(message).map(_.group(1).toLong))
      .getOrElse(throw new IllegalStateException("mutation result did not contain commitTs"))

  /** Rejects a non-empty directory so the demo never deletes arbitrary user data by itself. */
  private def requireEmptyDemoDirectory(path: Path): Unit =
    if Files.exists(path) then
      val stream = Files.list(path)
      try
        if stream.findAny().isPresent then
          throw new IllegalStateException(
            s"demo directory is not empty: $path. Use demo/run-demo.sh, which safely resets only repo-local .demo-data."
          )
      finally stream.close()

  /** Prints one feature-tour phase heading. */
  private def phase(milestone: String, capability: String): Unit =
    println()
    println("=" * 78)
    println(s"Milestone $milestone — $capability")
    println("=" * 78)

  /** Prints a top-level demo banner. */
  private def banner(text: String): Unit =
    println()
    println("#" * 78)
    println(s"# $text")
    println("#" * 78)
