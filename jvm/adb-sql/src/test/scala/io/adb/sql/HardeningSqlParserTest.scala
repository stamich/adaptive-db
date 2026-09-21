package io.adb.sql

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Regression tests for Milestone 2.0.1 SQL parser resource and narrowing limits. */
final class HardeningSqlParserTest:
  /** Verifies LIMIT cannot wrap a Long into a negative/truncated Int. */
  @Test def oversizedLimitIsRejected(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => new SqlParser().parse("SELECT * FROM t LIMIT 2147483648"))

  /** Verifies repeated EXPLAIN prefixes cannot cause unbounded parser recursion. */
  @Test def excessiveExplainNestingIsRejected(): Unit =
    val sql = List.fill(65)("EXPLAIN").mkString(" ") + " SELECT * FROM t"
    assertThrows(classOf[IllegalArgumentException], () => new SqlParser().parse(sql))

  /** Verifies parenthesis nesting is rejected by the tokenizer before parser stack growth. */
  @Test def excessiveParenthesisNestingIsRejected(): Unit =
    val sql = "(" * (Tokenizer.MaxParenthesisDepth + 1)
    assertThrows(classOf[IllegalArgumentException], () => Tokenizer.tokenize(sql))
