package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.*
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Binder tests of relational SELECTs: scopes, slots, aggregates and their error messages. */
final class SelectBinderTest:
  /** customer(id, name, city) and orders(id, customer_id, amount, note). */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("customer", Vector(("id", DataType.Int64, false), ("name", DataType.StringType, false), ("city", DataType.StringType, true)), "id")
    c.createEntity("orders", Vector(("id", DataType.Int64, false), ("customer_id", DataType.Int64, false), ("amount", DataType.Int64, false), ("note", DataType.StringType, true)), "id")
    c

  /** Binds `sql`, which must be a SELECT. */
  private def bind(sql: String): BoundSelect =
    new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]

  /** Asserts that binding `sql` fails with a message containing `fragment`. */
  private def fails(sql: String, fragment: String): Unit =
    val error = assertThrows(classOf[IllegalArgumentException], () => bind(sql))
    assertTrue(error.getMessage.contains(fragment), s"'${error.getMessage}' does not contain '$fragment'")

  /** Two instances of one entity get distinct slots for the same stored field. */
  @Test def selfJoinGetsDistinctSlots(): Unit =
    val s = bind("SELECT a.id, b.id FROM orders a JOIN orders b ON a.customer_id = b.customer_id")
    val Vector(a, b) = s.output.map(_.attribute)
    assertNotEquals(a.slot, b.slot)
    val ColumnOrigin.Stored(ra, ea, fa) = a.origin: @unchecked
    val ColumnOrigin.Stored(rb, eb, fb) = b.origin: @unchecked
    assertEquals(ea, eb)
    assertEquals(fa, fb)
    assertNotEquals(ra, rb)

  /** Slots are dense, allocated on first reference, and scans list only referenced columns. */
  @Test def slotsAreDenseAndColumnsPruned(): Unit =
    val s = bind("SELECT c.name, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id")
    val all = s.relations.flatMap(_.columns).map(_.slot.value).sorted
    assertEquals((0 until all.size).toVector, all)
    assertEquals(Set("c.name", "c.id"), s.base.columns.map(_.name).toSet)
    assertFalse(s.joins.head.relation.columns.exists(_.name == "o.note"))

  /** Name-resolution errors are reported precisely. */
  @Test def reportsNameErrors(): Unit =
    fails("SELECT id FROM customer c JOIN orders o ON o.customer_id = c.id", "ambiguous")
    fails("SELECT x.id FROM customer c", "unknown table or alias 'x'")
    fails("SELECT c.id FROM customer c JOIN orders c ON c.id = 1", "more than once")
    fails("SELECT c.id FROM customer c JOIN orders o ON o.id = z.id JOIN customer z ON z.id = 1", "not visible")
    fails("SELECT c.missing FROM customer c", "unknown column c.missing")

  /** The right side of a LEFT JOIN is nullable; the left side keeps its declared nullability. */
  @Test def leftJoinMakesRightSideNullable(): Unit =
    val s = bind("SELECT c.name, o.amount FROM customer c LEFT JOIN orders o ON o.customer_id = c.id")
    assertEquals(Vector(false, true), s.output.map(_.attribute.nullable))
    assertEquals(JoinType.Left, s.joins.head.joinType)

  /** GROUP BY rules, aggregate result types and aggregate reuse. */
  @Test def bindsAggregates(): Unit =
    val s = bind(
      "SELECT c.city, COUNT(*) AS n, SUM(o.amount) AS total, AVG(o.amount), SUM(o.amount) " +
        "FROM customer c JOIN orders o ON o.customer_id = c.id GROUP BY c.city ORDER BY total DESC"
    )
    assertEquals(Vector("city", "n", "total", "avg", "sum"), s.output.map(_.name))
    assertEquals(3, s.aggregates.size, "the repeated SUM(o.amount) is computed once")
    assertEquals(DataType.Int64, s.output(1).attribute.dataType)
    assertFalse(s.output(1).attribute.nullable)
    assertEquals(DataType.Float64, s.output(3).attribute.dataType)
    assertEquals(s.output(2).attribute, s.orderBy.head.attribute)

    fails("SELECT c.name, COUNT(*) FROM customer c GROUP BY c.city", "must appear in GROUP BY")
    fails("SELECT SUM(name) FROM customer", "not defined for STRING")
    fails("SELECT * FROM customer GROUP BY city", "SELECT *")
    fails("SELECT id FROM customer WHERE COUNT(*) > 1", "not allowed in WHERE")

  /** ORDER BY may use an aggregate that is not selected: it is computed but not projected. */
  @Test def orderByHiddenAggregate(): Unit =
    val s = bind("SELECT city FROM customer GROUP BY city ORDER BY COUNT(*) DESC")
    assertEquals(1, s.output.size)
    assertEquals(1, s.aggregates.size)
    assertEquals(s.aggregates.head.output, s.orderBy.head.attribute)

  /** SELECT * over a join qualifies the output names; LIMIT is bounded. */
  @Test def starAndLimit(): Unit =
    val s = bind("SELECT * FROM customer c CROSS JOIN orders o")
    assertEquals(7, s.output.size)
    assertEquals("c.id", s.output.head.name)
    fails(s"SELECT * FROM customer LIMIT ${Binder.MaxLimit + 1}", "LIMIT")
