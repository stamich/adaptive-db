package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.DataType
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Documents `BinderTest` and its role in the Milestone 2.0.1 JVM control plane. */
class BinderTest:
  /** Documents `bindsColumnsToFieldIds` and its role in the Milestone 2.0.1 JVM control plane. */
  @Test def bindsColumnsToFieldIds(): Unit =
    val catalog = new InMemoryCatalog
    catalog.createEntity("account", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false)), "id")
    val bound = new Binder(catalog).bind(new SqlParser().parse("SELECT balance FROM account WHERE id = 7"))
    assertTrue(bound.isInstanceOf[BoundSelect])
