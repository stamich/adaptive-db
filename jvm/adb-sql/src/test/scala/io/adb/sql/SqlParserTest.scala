package io.adb.sql

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Unit tests of [[SqlParser]]. */
class SqlParserTest:
  /** Parser under test. */
  private val parser = new SqlParser

  /** A SELECT with WHERE and LIMIT parses to a [[Select]]. */
  @Test def parsesSelect(): Unit =
    val stmt = parser.parse("SELECT id, balance FROM account WHERE balance > 100 LIMIT 10;")
    assertTrue(stmt.isInstanceOf[Select])

  /** CREATE TABLE parses its name and every column definition. */
  @Test def parsesCreate(): Unit =
    val stmt = parser.parse("CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL);")
    val create = stmt.asInstanceOf[CreateTable]
    assertEquals("account", create.name)
    assertEquals(2, create.columns.size)

  /** `ANALYZE` with and without a table, and `SET name = value` / `SET name TO value`. */
  @Test def parsesAnalyzeAndSet(): Unit =
    assertEquals(Analyze(Some("orders")), parser.parse("ANALYZE orders;"))
    assertEquals(Analyze(None), parser.parse("analyze"))
    assertEquals(SetOption("optimizer", "rule"), parser.parse("SET optimizer = rule"))
    assertEquals(SetOption("optimizer", "cost"), parser.parse("SET optimizer TO cost;"))
    assertEquals(Explain(Analyze(Some("t")), true), parser.parse("EXPLAIN ANALYZE ANALYZE t"))
    assertThrows(classOf[IllegalArgumentException], () => parser.parse("SET optimizer"))

  /** `SET SESSION CONSISTENCY` takes four levels, the bounded one with a unit. */
  @Test def parsesSessionConsistency(): Unit =
    assertEquals(SetSessionConsistency("STRONG", None), parser.parse("SET SESSION CONSISTENCY STRONG"))
    assertEquals(SetSessionConsistency("READ_YOUR_WRITES", None), parser.parse("set session consistency read_your_writes;"))
    assertEquals(SetSessionConsistency("EVENTUAL", None), parser.parse("SET SESSION CONSISTENCY EVENTUAL"))
    assertEquals(SetSessionConsistency("BOUNDED", Some((250L, "MS"))), parser.parse("SET SESSION CONSISTENCY BOUNDED 250 MS"))
    assertEquals(SetSessionConsistency("BOUNDED", Some((3L, "COMMITS"))), parser.parse("SET SESSION CONSISTENCY bounded 3 commits"))
    // A setting simply called `session` is still an ordinary option.
    assertEquals(SetOption("session", "x"), parser.parse("SET session = x"))
    for bad <- Seq("SET SESSION CONSISTENCY", "SET SESSION CONSISTENCY SOMETIMES", "SET SESSION CONSISTENCY BOUNDED",
        "SET SESSION CONSISTENCY BOUNDED 5", "SET SESSION CONSISTENCY BOUNDED 5 DAYS", "SET SESSION CONSISTENCY BOUNDED 1.5 MS",
        "SET SESSION CONSISTENCY BOUNDED -1 MS", "SET SESSION CONSISTENCY BOUNDED 99999999999999999999 MS") do
      assertThrows(classOf[IllegalArgumentException], () => { parser.parse(bad); () }, bad)

  /** A foreign-key hint must say NOT ENFORCED and may appear once per column. */
  @Test def parsesForeignKeyHints(): Unit =
    val create = parser.parse(
      "CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL REFERENCES customer(id) NOT ENFORCED)"
    ).asInstanceOf[CreateTable]
    assertEquals(Some(ColumnReference("customer", "id")), create.columns(1).references)
    assertFalse(create.columns(1).nullable)
    assertEquals(None, create.columns(0).references)
    val missing = assertThrows(classOf[IllegalArgumentException], () => parser.parse("CREATE TABLE o (id BIGINT PRIMARY KEY, c BIGINT REFERENCES customer(id))"))
    assertTrue(missing.getMessage.contains("NOT ENFORCED"))
    assertThrows(classOf[IllegalArgumentException], () =>
      parser.parse("CREATE TABLE o (id BIGINT PRIMARY KEY, c BIGINT REFERENCES a(id) NOT ENFORCED REFERENCES b(id) NOT ENFORCED)"))
