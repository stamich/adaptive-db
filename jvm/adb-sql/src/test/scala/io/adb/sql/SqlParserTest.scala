package io.adb.sql

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Documents `SqlParserTest` and its role in the Milestone 2.0.1 JVM control plane. */
class SqlParserTest:
  private val parser = new SqlParser

  /** Documents `parsesSelect` and its role in the Milestone 2.0.1 JVM control plane. */
  @Test def parsesSelect(): Unit =
    val stmt = parser.parse("SELECT id, balance FROM account WHERE balance > 100 LIMIT 10;")
    assertTrue(stmt.isInstanceOf[Select])

  /** Documents `parsesCreate` and its role in the Milestone 2.0.1 JVM control plane. */
  @Test def parsesCreate(): Unit =
    val stmt = parser.parse("CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL);")
    val create = stmt.asInstanceOf[CreateTable]
    assertEquals("account", create.name)
    assertEquals(2, create.columns.size)
