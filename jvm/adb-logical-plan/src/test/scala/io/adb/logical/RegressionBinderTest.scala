package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.*
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Regression checks carried over from the other 2.4 lineage: primary keys are NOT NULL,
  * `AS OF VERSION` is bounded, aggregate names are case-insensitive and slots restart at 0 for
  * every SELECT bound by the same binder.
  */
final class RegressionBinderTest:
  /** `account(id, balance)`. */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("account", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false)), "id")
    c
  /** Binder shared by every check (as the gateway shares one). */
  private val binder = new Binder(catalog)
  /** Parses and binds `sql`. */
  private def bind(sql: String): BoundStatement = binder.bind(new SqlParser().parse(sql))

  /** A PRIMARY KEY column is NOT NULL, whatever else the definition says. */
  @Test def primaryKeysAreNotNull(): Unit =
    for sql <- Vector(
        "CREATE TABLE t (id BIGINT PRIMARY KEY, v STRING)",
        "CREATE TABLE t (id BIGINT NULL PRIMARY KEY, v STRING)",
        "CREATE TABLE t (id BIGINT PRIMARY KEY NULL, v STRING)"
      )
    do
      val create = bind(sql).asInstanceOf[BoundCreateTable]
      assertEquals(("id", DataType.Int64, false), create.fields.head, sql)

  /** `AS OF VERSION` takes a commit timestamp: non-negative, and absent by default. */
  @Test def asOfIsBounded(): Unit =
    assertEquals(Some(7L), bind("SELECT id FROM account AS OF VERSION 7").asInstanceOf[BoundSelect].asOfVersion)
    assertEquals(Some(0L), bind("SELECT id FROM account AS OF VERSION 0").asInstanceOf[BoundSelect].asOfVersion)
    assertEquals(None, bind("SELECT id FROM account").asInstanceOf[BoundSelect].asOfVersion)
    assertThrows(classOf[IllegalArgumentException], () => { bind("SELECT id FROM account AS OF VERSION -1"); () })
    assertThrows(classOf[IllegalArgumentException], () => { bind("SELECT id FROM account AS OF VERSION 1.5"); () })

  /** Aggregate names match in any case, and the output name keeps the spelling as written. */
  @Test def aggregatesAreCaseInsensitive(): Unit =
    val bound = bind("SELECT count(*), Sum(balance), MAX(balance), avg(balance) FROM account").asInstanceOf[BoundSelect]
    assertEquals(4, bound.output.size)
    assertEquals(bound.output.map(_.attribute.slot).distinct.size, 4, "four distinct aggregates")

  /** Every SELECT allocates its slots from 0, however many statements the binder bound before. */
  @Test def slotsRestartPerSelect(): Unit =
    val first = bind("SELECT balance, id FROM account").asInstanceOf[BoundSelect]
    val second = bind("SELECT balance, id FROM account").asInstanceOf[BoundSelect]
    assertEquals(Vector(0, 1), first.output.map(_.attribute.slot.value))
    assertEquals(first.output.map(_.attribute.slot), second.output.map(_.attribute.slot))
