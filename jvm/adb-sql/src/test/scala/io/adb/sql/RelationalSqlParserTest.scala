package io.adb.sql

import io.adb.sql.SqlExpr.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Parser tests of the 2.1 SELECT grammar: joins, aliases, qualified columns, aggregates,
  * GROUP BY, ORDER BY and their budgets.
  */
final class RelationalSqlParserTest:
  /** Parser under test. */
  private val parser = new SqlParser

  /** Parses `sql`, which must be a SELECT. */
  private def select(sql: String): Select = parser.parse(sql).asInstanceOf[Select]

  /** Joins keep their kind, alias and ON condition; qualified columns keep their qualifier. */
  @Test def parsesJoinsWithAliases(): Unit =
    val s = select(
      "SELECT c.name, o.amount FROM customer c JOIN orders AS o ON o.customer_id = c.id " +
        "LEFT OUTER JOIN region r ON r.id = c.region_id CROSS JOIN tag"
    )
    assertEquals(TableRef("customer", Some("c")), s.from.base)
    assertEquals(Vector(JoinKind.Inner, JoinKind.Left, JoinKind.Cross), s.from.joins.map(_.kind))
    assertEquals(Some("o"), s.from.joins(0).table.alias)
    assertEquals(
      Some(Binary(Column("customer_id", Some("o")), SqlBinaryOp.Eq, Column("id", Some("c")))),
      s.from.joins(0).on
    )
    assertEquals(None, s.from.joins(2).on)
    assertEquals(SelectItem(Column("name", Some("c")), None), s.items(0))

  /** Aggregates, aliases, GROUP BY, ORDER BY and LIMIT are parsed in order. */
  @Test def parsesAggregationAndOrdering(): Unit =
    val s = select(
      "SELECT region, COUNT(*) AS n, SUM(amount) total FROM orders " +
        "WHERE amount > 0 GROUP BY region ORDER BY total DESC, region LIMIT 5;"
    )
    assertEquals(SelectItem(AggregateCall(SqlAggregate.Count, None), Some("n")), s.items(1))
    assertEquals(SelectItem(AggregateCall(SqlAggregate.Sum, Some(Column("amount"))), Some("total")), s.items(2))
    assertEquals(Vector(Column("region")), s.groupBy)
    assertEquals(Vector(OrderItem(Column("total"), true), OrderItem(Column("region"), false)), s.orderBy)
    assertEquals(Some(5), s.limit)

  /** `AS OF VERSION` still follows the FROM clause, with or without aliases and joins. */
  @Test def parsesSnapshotAfterFrom(): Unit =
    assertEquals(Some(3L), select("SELECT * FROM account AS OF VERSION 3 WHERE id = 1").asOfVersion)
    val aliased = select("SELECT a.id FROM account a JOIN account b ON a.id = b.id AS OF VERSION 9")
    assertEquals(Some(9L), aliased.asOfVersion)
    assertEquals(Some("b"), aliased.from.joins.head.table.alias)
    assertEquals(Some("x"), select("SELECT x.id FROM account AS x").from.base.alias)

  /** Invalid aggregate calls and oversized statements are rejected. */
  @Test def rejectsInvalidInput(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => parser.parse("SELECT SUM(*) FROM t"))
    assertThrows(classOf[IllegalArgumentException], () => parser.parse("SELECT * FROM t JOIN u"))
    // Unsupported join kinds must not be misread as an alias followed by an INNER JOIN.
    for kind <- Vector("RIGHT", "RIGHT OUTER", "FULL", "NATURAL") do
      val error = assertThrows(classOf[IllegalArgumentException], () => parser.parse(s"SELECT * FROM a $kind JOIN b ON a.id = b.id"))
      assertTrue(error.getMessage.contains("not supported"), error.getMessage)
    val tooManyJoins = (1 to SqlParser.MaxJoins + 1).map(i => s"CROSS JOIN t$i").mkString("SELECT * FROM t0 ", " ", "")
    assertThrows(classOf[IllegalArgumentException], () => parser.parse(tooManyJoins))
