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
