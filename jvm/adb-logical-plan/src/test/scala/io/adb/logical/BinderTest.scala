package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.DataType
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
