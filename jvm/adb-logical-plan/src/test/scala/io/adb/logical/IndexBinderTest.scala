package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.*
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Binding of the index statements against the catalog. */
final class IndexBinderTest:
  /** account(id, balance, owner). */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("account", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false), ("owner", DataType.StringType, false)), "id")
    c
  /** Binds `sql`. */
  private def bind(sql: String): BoundStatement = new Binder(catalog).bind(new SqlParser().parse(sql))

  /** Columns resolve to fields (case-insensitive) with their directions. */
  @Test def bindsCreateIndex(): Unit =
    bind("CREATE INDEX idx ON Account (BALANCE DESC) INCLUDE (owner)") match
      case BoundCreateIndex(name, entity, columns, include) =>
        assertEquals("idx", name)
        assertEquals("account", entity.name)
        assertEquals(Vector("balance" -> true), columns.map((f, d) => f.name -> d))
        assertEquals(Vector("owner"), include.map(_.name))
      case other => fail(s"unexpected $other")
    assertEquals(BoundDropIndex("idx"), bind("DROP INDEX idx"))
    assertEquals(BoundShowIndexes(None), bind("SHOW INDEXES"))
    assertTrue(bind("SHOW INDEXES ON account").asInstanceOf[BoundShowIndexes].entity.isDefined)

  /** Unknown tables or columns and repeated columns are refused. */
  @Test def rejectsInvalidIndexes(): Unit =
    for bad <- Vector(
        "CREATE INDEX i ON missing (balance)",
        "CREATE INDEX i ON account (missing)",
        "CREATE INDEX i ON account (balance) INCLUDE (balance)",
        "SHOW INDEXES ON missing"
      )
    do assertThrows(classOf[IllegalArgumentException], () => { bind(bad); () }, bad)
