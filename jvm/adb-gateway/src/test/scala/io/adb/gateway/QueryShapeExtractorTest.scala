package io.adb.gateway

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.{Binder, BoundCreateTable, BoundSelect}
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The semantic shape of bound SELECTs. */
final class QueryShapeExtractorTest:
  /** A catalog with `customer(id, name, city)` and `orders(id, customer_id, amount, status)`. */
  private val catalog = new InMemoryCatalog
  private val parser = new SqlParser
  private val binder = new Binder(catalog)
  Vector(
    "CREATE TABLE customer (id BIGINT PRIMARY KEY, name STRING, city STRING);",
    "CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL, status STRING);"
  ).foreach { ddl =>
    binder.bind(parser.parse(ddl)) match
      case c: BoundCreateTable => catalog.createEntity(c.name, c.fields, c.primaryKey, c.references)
      case _ => fail("expected CREATE TABLE")
  }

  /** Binds and extracts the shape of `sql`. */
  private def shape(sql: String) = QueryShapeExtractor.extract(binder.bind(parser.parse(sql)).asInstanceOf[BoundSelect])
  /** Entity id of `name`. */
  private def entity(name: String) = catalog.entity(name).get.id.value
  /** Field id of `entity.column`. */
  private def field(table: String, column: String) = catalog.entity(table).get.field(column).get.id.value

  /** Literals, layout and projection do not change the shape. */
  @Test def literalsDoNotMatter(): Unit =
    val a = shape("SELECT name FROM customer WHERE city = 'Gdansk'")
    val b = shape("select   id FROM customer WHERE city = 'Warsaw' -- other")
    assertEquals(a, b)
    assertEquals(a.id, b.id)
    assertEquals(Vector(entity("customer")), a.entities)
    assertEquals(1, a.filterFields.size)

  /** A join is an edge between the stored columns, whichever side is written first. */
  @Test def joinEdgesAreNormalised(): Unit =
    val a = shape("SELECT c.name, o.amount FROM customer c JOIN orders o ON c.id = o.customer_id WHERE o.status = 'open' ORDER BY o.amount DESC LIMIT 10")
    val b = shape("SELECT c.name, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id WHERE o.status = 'closed' ORDER BY o.amount DESC LIMIT 5")
    assertEquals(a, b)
    assertEquals(1, a.joinEdges.size)
    val e = a.joinEdges.head
    assertEquals(Set(entity("customer"), entity("orders")), Set(e.leftEntity, e.rightEntity))
    assertTrue(a.hasLimit && !a.hasAggregate)
    assertEquals(Vector(true), a.orderFields.map(_.descending))
    assertEquals(field("orders", "status"), a.filterFields.head.field)

  /** Different filters, order direction, aggregation or joins give different ids. */
  @Test def differentShapesDiffer(): Unit =
    val base = shape("SELECT id FROM orders WHERE customer_id = 1")
    assertNotEquals(base.id, shape("SELECT id FROM orders WHERE status = 'a'").id)
    assertNotEquals(base.id, shape("SELECT id FROM orders WHERE customer_id = 1 ORDER BY amount").id)
    assertNotEquals(base.id, shape("SELECT id FROM orders WHERE customer_id = 1 LIMIT 3").id)
    assertNotEquals(shape("SELECT status FROM orders ORDER BY amount").id, shape("SELECT status FROM orders ORDER BY amount DESC").id)
    val agg = shape("SELECT status, COUNT(*) AS n FROM orders GROUP BY status")
    assertTrue(agg.hasAggregate)
    assertNotEquals(base.id, agg.id)
