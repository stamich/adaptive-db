package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.{DataType, ForeignKeyRef}
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Unit tests of [[Binder]]. */
class BinderTest:
  /** A SELECT with a WHERE clause binds to a [[BoundSelect]]. */
  @Test def bindsColumnsToFieldIds(): Unit =
    val catalog = new InMemoryCatalog
    catalog.createEntity("account", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false)), "id")
    val bound = new Binder(catalog).bind(new SqlParser().parse("SELECT balance FROM account WHERE id = 7"))
    assertTrue(bound.isInstanceOf[BoundSelect])

  /** Foreign-key hints bind to the referenced primary key; ANALYZE and SET bind to their targets. */
  @Test def bindsForeignKeysAnalyzeAndSet(): Unit =
    val catalog = new InMemoryCatalog
    val customer = catalog.createEntity("customer", Vector(("id", DataType.Int64, false), ("name", DataType.StringType, false)), "id")
    val binder = new Binder(catalog)
    val parser = new SqlParser
    val create = binder.bind(parser.parse(
      "CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT REFERENCES customer(id) NOT ENFORCED)"
    )).asInstanceOf[BoundCreateTable]
    assertEquals(Map("customer_id" -> ForeignKeyRef(customer.id, customer.primaryKey)), create.references)
    val orders = catalog.createEntity(create.name, create.fields, create.primaryKey, create.references)
    assertEquals(Some(ForeignKeyRef(customer.id, customer.primaryKey)), orders.field("customer_id").get.references)

    for bad <- Vector(
        "CREATE TABLE o (id BIGINT PRIMARY KEY, c BIGINT REFERENCES nobody(id) NOT ENFORCED)",
        "CREATE TABLE o (id BIGINT PRIMARY KEY, c BIGINT REFERENCES customer(name) NOT ENFORCED)",
        "CREATE TABLE o (id BIGINT PRIMARY KEY, c STRING REFERENCES customer(id) NOT ENFORCED)",
        "CREATE TABLE o (id BIGINT PRIMARY KEY, c BIGINT REFERENCES o(id) NOT ENFORCED)",
        "ANALYZE nobody",
        "SET optimizer = fast",
        "SET planner = cost"
      )
    do assertThrows(classOf[IllegalArgumentException], () => binder.bind(parser.parse(bad)), bad)

    assertEquals(BoundAnalyze(Vector(customer)), binder.bind(parser.parse("ANALYZE customer")))
    assertEquals(BoundAnalyze(catalog.entities), binder.bind(parser.parse("ANALYZE")))
    assertEquals(BoundSetOptimizer(OptimizerMode.Rule), binder.bind(parser.parse("SET optimizer = RULE")))
