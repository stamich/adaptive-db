package io.adb.cli

import io.adb.catalog.FileCatalog
import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import java.nio.file.{Files, Path}
import scala.jdk.CollectionConverters.*

/**
 * Runs the Adaptive DB feature tour (Milestone 2.2.3).
 *
 * The demo executes on one current engine while presenting features in the order in which they
 * entered the project: WAL/recovery, persistent current storage, persistent history, native
 * batch execution, the Scala SQL/control plane, relational execution (joins, aggregation,
 * sorting) with explained planning decisions and runtime profiles, and finally statistics and
 * cost-based optimization.
 */
object DemoMain:
  /** Extracts the commit timestamp from a mutation result message. */
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

    banner("Adaptive DB 2.2.3 - chronological feature tour")
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

      relationalPhase(db)
      statisticsPhase(db)
    }

    banner("Demo completed successfully")
    println("Re-run demo/run-demo.sh to recreate the database from scratch.")

  /** Milestone 2.1.3: joins, aggregation, sorting, explained decisions and runtime profiles. */
  private def relationalPhase(db: AdaptiveDatabase): Unit =
    phase("2.1.3", "relational execution: joins, GROUP BY, ORDER BY, TopK, explained decisions")
    execute(db, "CREATE TABLE customer (id BIGINT PRIMARY KEY, name STRING NOT NULL, city STRING);")
    execute(db, "CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL);")
    for (id, name, city) <- Vector((1, "Ada", "Krakow"), (2, "Ben", "Gdansk"), (3, "Cy", "Krakow"), (4, "Dee", "Poznan")) do
      execute(db, s"INSERT INTO customer VALUES ($id, '$name', '$city');")
    for (id, customer, amount) <- Vector((10, 1, 120), (11, 1, 80), (12, 2, 300), (13, 3, 40), (14, 3, 60), (15, 3, 500)) do
      execute(db, s"INSERT INTO orders VALUES ($id, $customer, $amount);")

    execute(db, "SELECT c.name, o.id, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id ORDER BY o.amount DESC;")
    println("Feature shown: HashJoin over slot-addressed rows; ORDER BY runs as a native Sort.")

    execute(db, "SELECT c.name, COUNT(o.id) AS orders FROM customer c LEFT JOIN orders o ON o.customer_id = c.id GROUP BY c.name ORDER BY c.name;")
    println("Feature shown: LEFT JOIN null-fills customers without orders (Dee); COUNT(x) skips the NULLs.")

    execute(db, "SELECT c.city, SUM(o.amount) AS total, AVG(o.amount) FROM customer c JOIN orders o ON o.customer_id = c.id GROUP BY c.city ORDER BY total DESC LIMIT 2;")
    println("Feature shown: GROUP BY with checked SUM and AVG; LIMIT over ORDER BY runs as TopK.")

    execute(db, "SELECT a.id, b.id FROM orders a JOIN orders b ON a.customer_id = b.customer_id AND a.id < b.id;")
    println("Feature shown: a self-join; both instances of 'orders' read field ids into different slots.")

    execute(db, "SELECT c.name, o.id, o.amount FROM customer c JOIN orders o ON o.customer_id <> c.id AND o.amount >= 300 ORDER BY c.name, o.id;")
    println("Feature shown: without an equality key the join falls back to the bounded NestedLoopJoin; o.amount >= 300 is pushed below it.")

    execute(db, "EXPLAIN ANALYZE SELECT c.city, SUM(o.amount) AS total FROM customer c JOIN orders o ON o.customer_id = c.id WHERE o.amount >= 50 GROUP BY c.city ORDER BY total DESC LIMIT 2;")
    println("Feature shown: EXPLAIN ANALYZE prints the optimized plan, the planner's decisions with reasons, and the native per-operator profile.")

  /** Milestone 2.2.3: ANALYZE, foreign-key hints, estimates, join ordering, cost vs rule mode. */
  private def statisticsPhase(db: AdaptiveDatabase): Unit =
    phase("2.2.3", "statistics + cost-based optimizer: ANALYZE, estimates, join order, q-error")
    execute(db, "CREATE TABLE region (id BIGINT PRIMARY KEY, name STRING NOT NULL);")
    execute(db, "CREATE TABLE shop (id BIGINT PRIMARY KEY, region_id BIGINT NOT NULL REFERENCES region(id) NOT ENFORCED, name STRING NOT NULL);")
    execute(db, "CREATE TABLE sale (id BIGINT PRIMARY KEY, shop_id BIGINT NOT NULL REFERENCES shop(id) NOT ENFORCED, amount BIGINT NOT NULL);")
    println("Feature shown: REFERENCES ... NOT ENFORCED declares a foreign key the engine never checks; the optimizer uses it.")
    for id <- 1 to 4 do db.execute(s"INSERT INTO region VALUES ($id, 'region-$id');")
    for id <- 1 to 40 do db.execute(s"INSERT INTO shop VALUES ($id, ${1 + id % 4}, 'shop-$id');")
    for id <- 1 to 400 do db.execute(s"INSERT INTO sale VALUES ($id, ${1 + (id * 7) % 40}, ${(id * 37) % 500 + 1});")
    println("Loaded 4 regions, 40 shops and 400 sales (single-row INSERTs, not echoed).")

    val query = "SELECT r.name, SUM(s.amount) AS total FROM sale s JOIN shop sh ON s.shop_id = sh.id " +
      "JOIN region r ON sh.region_id = r.id WHERE r.name = 'region-2' GROUP BY r.name"
    execute(db, s"EXPLAIN $query;")
    println("Feature shown: without statistics every table is assumed to hold 1,000 rows; EXPLAIN warns and suggests ANALYZE.")

    execute(db, "ANALYZE;")
    execute(db, s"EXPLAIN ANALYZE $query;")
    println("Feature shown: with statistics the joins are reordered (the selective region filter first), every operator")
    println("carries an estimate, and EXPLAIN ANALYZE compares it with the native profile (q-error).")

    execute(db, s"$query;")
    execute(db, "SET optimizer = rule;")
    execute(db, s"$query;")
    execute(db, "SET optimizer = cost;")
    println("Feature shown: SET optimizer = rule restores the 2.1.3 planner (SQL join order); both modes return the same rows.")

    for id <- 401 to 600 do db.execute(s"INSERT INTO sale VALUES ($id, ${1 + id % 40}, 1);")
    execute(db, "EXPLAIN SELECT COUNT(*) FROM sale;")
    println("Feature shown: after 200 more sales (50% of the analyzed rows) the statistics of 'sale' are reported stale.")

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
    println(s"Milestone $milestone - $capability")
    println("=" * 78)

  /** Prints a top-level demo banner. */
  private def banner(text: String): Unit =
    println()
    println("#" * 78)
    println(s"# $text")
    println("#" * 78)
