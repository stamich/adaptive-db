package io.adb.cli

import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, QueryResult}
import java.nio.file.{Files, Path}
import scala.jdk.CollectionConverters.*

/**
 * Runs the Adaptive DB feature tour (Milestone 2.4.0).
 *
 * The demo executes on one current engine while presenting features in the order in which they
 * entered the project: WAL/recovery, persistent current storage, persistent history, native
 * batch execution, the Scala SQL/control plane, relational execution (joins, aggregation,
 * sorting) with explained planning decisions and runtime profiles, statistics and cost-based
 * optimization, secondary indexes as MVCC-versioned projections chosen by cost, and finally
 * schema evolution: versioned schemas owned by the engine, changed without rewriting rows.
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

    banner("Adaptive DB 2.4.0 - chronological feature tour")
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
      indexPhase(db)
      schemaPhase(db)
    }

    phase("2.4.0", "schemas survive a restart")
    println("The native database was closed again; the schemas are replayed from the log with the rows.")
    withDatabase(dataDir, nativeLib) { db =>
      execute(db, "SHOW SCHEMA ledger;")
      execute(db, "SELECT id, amount, currency FROM ledger WHERE id <= 3;")
      println("Feature shown: the engine catalog is durable with the data; the JVM keeps no catalog file.")
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

  /** Milestone 2.3.0: secondary indexes, covering scans, row fetches, sort elision, `AS OF`
    * through an index, and the cost-based choice between index and sequential scans.
    */
  private def indexPhase(db: AdaptiveDatabase): Unit =
    phase("2.3.0", "secondary indexes: CREATE INDEX, covering scans, ordered LIMIT, AS OF, cost-based access paths")
    execute(db, "CREATE TABLE ledger (id BIGINT PRIMARY KEY, account BIGINT NOT NULL, amount BIGINT NOT NULL, memo STRING NOT NULL);")
    // amount = id * 7919 mod 10007 is unique for these ids, so ORDER BY amount has no ties and
    // every plan returns the same rows.
    for id <- 1 to 2000 do db.execute(s"INSERT INTO ledger VALUES ($id, ${id % 200}, ${(id * 7919L) % 10007}, 'memo-$id');")
    println("Loaded 2,000 ledger rows over 200 accounts (single-row INSERTs, not echoed).")
    execute(db, "ANALYZE ledger;")
    val byAccount = "SELECT account, amount FROM ledger WHERE account = 42"
    execute(db, s"EXPLAIN $byAccount;")
    println("Feature shown: without an index the only access path is the sequential scan of the table.")

    execute(db, "CREATE INDEX idx_ledger_account ON ledger (account) INCLUDE (amount);")
    execute(db, "SHOW INDEXES;")
    println("Feature shown: the build reads the table at one snapshot and publishes the index with a checkpoint;")
    println("later commits maintain it in the same commit as the row, so it is never stale.")

    execute(db, "ANALYZE ledger;")
    execute(db, s"EXPLAIN ANALYZE $byAccount;")
    println("Feature shown: the cost model picks the covering index scan (no row fetch); EXPLAIN names the index,")
    println("prints its bounds by column, lists the rejected sequential scan and the index statistics.")
    execute(db, "EXPLAIN SELECT id, memo FROM ledger WHERE account = 42;")
    println("Feature shown: columns the index does not hold are read by a batched RowFetch in index order.")

    execute(db, "CREATE INDEX idx_ledger_amount ON ledger (amount);")
    val top = "SELECT id, amount FROM ledger ORDER BY amount DESC LIMIT 5"
    execute(db, s"EXPLAIN $top;")
    execute(db, s"$top;")
    execute(db, "SET optimizer = rule;")
    execute(db, s"$top;")
    execute(db, "SET optimizer = cost;")
    println("Feature shown: the index read backward delivers ORDER BY ... DESC; LIMIT stops after 5 entries instead")
    println("of a TopK over every row. Rule mode (no indexes) returns the same rows.")

    val before = commitTs(execute(db, "UPDATE ledger SET amount = 20000 WHERE id = 7;"))
    execute(db, s"$top;")
    execute(db, s"SELECT id, amount FROM ledger AS OF VERSION ${before - 1} ORDER BY amount DESC LIMIT 5;")
    println(s"Feature shown: index entries are versioned like rows; AS OF VERSION ${before - 1} reads the index as it was")
    println("before the update, so history queries use indexes too (until VACUUM removes the old entries).")

    execute(db, "DROP INDEX idx_ledger_amount;")
    execute(db, s"EXPLAIN $top;")
    println("Feature shown: after DROP INDEX the planner falls back to the sequential scan with a TopK.")

  /** Milestone 2.4.0: schema evolution. `ALTER TABLE` publishes a new schema version at a new
    * catalog epoch without rewriting rows; old rows read defaults ("virtual defaults") and are
    * migrated when written; RESTRICT protects indexed columns; stale plans are refused.
    */
  private def schemaPhase(db: AdaptiveDatabase): Unit =
    phase("2.4.0", "schema evolution: ALTER TABLE, virtual defaults, lazy migration, RESTRICT, SHOW SCHEMA")
    execute(db, "ALTER TABLE ledger ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR';")
    execute(db, "SELECT id, amount, currency FROM ledger WHERE id <= 3;")
    println("Feature shown: the 2,000 existing rows were not rewritten; they read the column's default.")

    execute(db, "EXPLAIN SELECT id FROM ledger WHERE currency = 'EUR';")
    println("Feature shown: the Schema section names the catalog epoch the plan carries, and the estimate for the")
    println("new column comes from its default (every analyzed row reads 'EUR') instead of a guessed selectivity.")

    execute(db, "UPDATE ledger SET currency = 'PLN' WHERE id = 2;")
    execute(db, "INSERT INTO ledger (id, account, amount, memo) VALUES (2001, 1, 9999, 'memo-2001');")
    execute(db, "SELECT id, currency FROM ledger WHERE id = 2 OR id = 3 OR id = 2001;")
    println("Feature shown: a written row is migrated to the latest version (lazy migration); an INSERT may omit")
    println("a NOT NULL column that has a default.")

    execute(db, "ALTER TABLE ledger RENAME COLUMN memo TO note;")
    execute(db, "SELECT id, note FROM ledger WHERE id = 1;")
    println("Feature shown: a rename changes only metadata; the field id and the stored values stay.")

    try execute(db, "ALTER TABLE ledger DROP COLUMN account;")
    catch case error: IllegalArgumentException => println(s"refused: ${error.getMessage}")
    println("Feature shown: DROP COLUMN is RESTRICT; idx_ledger_account depends on the column.")
    execute(db, "ALTER TABLE ledger DROP COLUMN note;")
    execute(db, "SELECT * FROM ledger WHERE id = 1;")
    println("Feature shown: a dropped column disappears from reads, but its values stay in the log for history.")

    try execute(db, "INSERT INTO ledger VALUES (2002, 1, 'not a number', 'EUR');")
    catch case error: IllegalArgumentException => println(s"refused: ${error.getMessage}")
    execute(db, "SHOW SCHEMA ledger;")
    println("Feature shown: every row is validated against its schema; SHOW SCHEMA lists each version, its catalog")
    println("epoch, the change that produced it and the columns it has.")

    execute(db, "CREATE TABLE scratch (id BIGINT PRIMARY KEY, v BIGINT DEFAULT 0);")
    execute(db, "DROP TABLE scratch;")
    println("Feature shown: DDL commits through the engine's log like data; a plan made before any of it would be")
    println("refused with SCHEMA_CHANGED and planned again by the gateway.")

  /** Opens one native database with its engine catalog for a demo phase (importing a
    * pre-2.4.0 catalog file left by an older demo run).
    */
  private def withDatabase(dataDir: Path, nativeLib: Path)(body: AdaptiveDatabase => Unit): Unit =
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    try body(AdaptiveDatabase.open(native, Some(dataDir.resolve("catalog.properties"))))
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
