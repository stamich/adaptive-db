package io.adb.plancache

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Statement keys come from the token stream. */
final class StatementKeyTest:
  /** Whitespace and comments do not change the key. */
  @Test def layoutDoesNotMatter(): Unit =
    val a = StatementKey.of("SELECT  a FROM t -- note", "cost")
    val b = StatementKey.of("SELECT a\n  FROM   t", "cost")
    assertTrue(a.isDefined)
    assertEquals(a, b)
    assertEquals(a.get.hashCode, b.get.hashCode)

  /** A different literal, name or mode is a different key. */
  @Test def differentStatementsDiffer(): Unit =
    val base = StatementKey.of("SELECT a FROM t WHERE a = 1", "cost")
    assertNotEquals(base, StatementKey.of("SELECT a FROM t WHERE a = 2", "cost"))
    assertNotEquals(base, StatementKey.of("SELECT b FROM t WHERE a = 1", "cost"))
    assertNotEquals(base, StatementKey.of("SELECT a FROM t WHERE a = 1", "rule"))
    assertNotEquals(StatementKey.of("SELECT 'a b' FROM t", "cost"), StatementKey.of("SELECT 'a' 'b' FROM t", "cost"))

  /** Statements that do not lex, are empty, or are huge are not cacheable. */
  @Test def uncacheableStatementsGiveNoKey(): Unit =
    assertEquals(None, StatementKey.of("SELECT 'open", "cost"))
    assertEquals(None, StatementKey.of("   -- nothing", "cost"))
    val huge = "SELECT " + (1 to StatementKey.MaxTokens).map(_ => "a").mkString(" ")
    assertEquals(None, StatementKey.of(huge, "cost"))

  /** Longer statements weigh more. */
  @Test def weightGrowsWithTheStatement(): Unit =
    val short = StatementKey.of("SELECT a FROM t", "cost").get
    val long = StatementKey.of("SELECT a, b, c FROM t WHERE a = 1", "cost").get
    assertTrue(long.weight > short.weight && short.weight > 0)
