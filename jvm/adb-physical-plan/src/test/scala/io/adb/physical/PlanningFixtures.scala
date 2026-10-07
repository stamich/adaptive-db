package io.adb.physical

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.model.DataType
import io.adb.optimizer.RuleOptimizer
import io.adb.sql.SqlParser
import java.nio.file.{Files, Path}

/** Shared catalog and planning helpers of the physical-plan tests. */
object PlanningFixtures:
  /** customer(id, name, city) and orders(id, customer_id, amount). */
  def catalog(): InMemoryCatalog =
    val c = new InMemoryCatalog
    c.createEntity("customer", Vector(("id", DataType.Int64, false), ("name", DataType.StringType, false), ("city", DataType.StringType, true)), "id")
    c.createEntity("orders", Vector(("id", DataType.Int64, false), ("customer_id", DataType.Int64, false), ("amount", DataType.Int64, false)), "id")
    c

  /** Parses, binds, optimizes and physically plans `sql` with `policy`. */
  def planned(sql: String, policy: PlanningPolicy = DefaultPlanningPolicy): PlannedQuery =
    val bound = new Binder(catalog()).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    PhysicalPlanner.plan(new RuleOptimizer().optimize(LogicalPlanner.plan(bound)), policy)

  /** The query whose encoded plan is pinned in the cross-language contract fixture. */
  val ContractQuery: String =
    "SELECT c.name, SUM(o.amount) AS total FROM customer c JOIN orders o ON o.customer_id = c.id " +
      "LEFT JOIN orders big ON big.customer_id = c.id AND big.amount > 1000 " +
      "WHERE c.city = 'Kraków' AND o.amount >= 0 GROUP BY c.name ORDER BY total DESC LIMIT 3"

  /** Location of the contract fixture, found by walking up from the working directory. */
  def contractFixture: Path =
    Iterator
      .iterate(Path.of("").toAbsolutePath)(_.getParent)
      .takeWhile(_ != null)
      .map(_.resolve("crates/adb-plan-wire/tests/fixtures/jvm-relational-plan.json"))
      .find(Files.exists(_))
      .getOrElse(throw new IllegalStateException("contract fixture not found above the working directory"))
