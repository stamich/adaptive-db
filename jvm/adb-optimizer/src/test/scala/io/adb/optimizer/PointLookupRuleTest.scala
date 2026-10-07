package io.adb.optimizer

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.model.DataType
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Unit tests of [[PointLookupRule]]. */
class PointLookupRuleTest:
  /** An equality on the primary key is rewritten into a `PointLookup`. */
  @Test def rewritesPkEquality(): Unit =
    val catalog = new InMemoryCatalog
    catalog.createEntity("account", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false)), "id")
    val bound = new Binder(catalog).bind(new SqlParser().parse("SELECT * FROM account WHERE id = 7")).asInstanceOf[BoundSelect]
    val optimized = new RuleOptimizer().optimize(LogicalPlanner.plan(bound))
    assertTrue(optimized.toString.contains("PointLookup"))
