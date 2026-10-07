package io.adb.optimizer

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.DataType
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Optimizer tests: predicate pushdown through joins and point lookups below joins. */
final class PredicatePushdownRuleTest:
  /** customer(id, name) and orders(id, customer_id, amount). */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("customer", Vector(("id", DataType.Int64, false), ("name", DataType.StringType, false)), "id")
    c.createEntity("orders", Vector(("id", DataType.Int64, false), ("customer_id", DataType.Int64, false), ("amount", DataType.Int64, false)), "id")
    c

  /** The optimized logical plan of `sql`. */
  private def optimize(sql: String): LogicalPlan =
    val bound = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    new RuleOptimizer().optimize(LogicalPlanner.plan(bound))

  /** The first join of the plan. */
  private def join(plan: LogicalPlan): Join = plan match
    case join: Join => join
    case other => join(other.children.head)

  /** Single-side WHERE conjuncts move below an INNER join; a primary-key equality becomes a
    * point lookup on that side; a crossing conjunct joins the join condition.
    */
  @Test def innerJoinPushdown(): Unit =
    val plan = optimize(
      "SELECT c.name, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id " +
        "WHERE c.id = 7 AND o.amount > 10 AND o.id > c.id"
    )
    val j = join(plan)
    assertTrue(j.left.isInstanceOf[PointLookup], LogicalPlan.render(plan))
    assertTrue(j.right match { case Filter(TableScan(_), _) => true; case _ => false })
    assertEquals(2, TypedExpr.conjuncts(j.condition.get).size)
    assertFalse(plan.children.exists(_.isInstanceOf[Filter]), "nothing stays above the join")

  /** A CROSS JOIN with an equality in WHERE becomes an INNER join with that condition. */
  @Test def crossJoinWithWhereBecomesInner(): Unit =
    val j = join(optimize("SELECT c.name FROM customer c CROSS JOIN orders o WHERE o.customer_id = c.id"))
    assertEquals(JoinType.Inner, j.joinType)
    assertTrue(j.condition.isDefined)

  /** LEFT JOIN: WHERE on the preserved side is pushed, WHERE on the null-filled side stays
    * above the join; ON conditions on the right side are pushed into it, left-only ON
    * conditions stay in the join.
    */
  @Test def leftJoinPreservesNullFilling(): Unit =
    val plan = optimize(
      "SELECT c.name FROM customer c LEFT JOIN orders o ON o.customer_id = c.id AND o.amount > 5 AND c.id > 1 " +
        "WHERE c.name = 'ann' AND o.amount < 100"
    )
    val j = join(plan)
    assertEquals(JoinType.Left, j.joinType)
    assertTrue(j.left.isInstanceOf[Filter], "WHERE on the preserved side is pushed")
    assertTrue(j.right.isInstanceOf[Filter], "right-only ON conjunct is pushed into the right side")
    assertEquals(2, TypedExpr.conjuncts(j.condition.get).size, "equality and left-only ON conjunct stay")
    val aboveJoin = plan match
      case Project(Filter(_: Join, predicate), _) => Some(predicate)
      case _ => None
    assertTrue(aboveJoin.isDefined, "WHERE on the null-filled side stays above the join:\n" + LogicalPlan.render(plan))

  /** A primary-key equality among other conditions keeps those conditions as a filter. */
  @Test def pointLookupKeepsOtherConditions(): Unit =
    optimize("SELECT name FROM customer WHERE name = 'x' AND id = 3") match
      case Project(Filter(PointLookup(_, 3L), _), _) => ()
      case other => fail("unexpected plan:\n" + LogicalPlan.render(other))
