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
