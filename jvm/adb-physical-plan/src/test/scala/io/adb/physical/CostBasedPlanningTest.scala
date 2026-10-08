package io.adb.physical

import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.{EngineLimits, OptimizerConfig, RuleOptimizer}
import io.adb.optimizer.cardinality.CardinalityEstimator
import io.adb.optimizer.cost.CostModel
import io.adb.physical.PhysicalPlan.*
import io.adb.sql.SqlParser
import io.adb.statistics.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The cost-based policy: build sides, LEFT JOIN, limits, TopK, plan annotations and warnings. */
final class CostBasedPlanningTest:
  /** Catalog of the planning fixtures (customer, orders). */
  private val catalog = PlanningFixtures.catalog()
  /** 1,000 customers and 10 orders. */
  private val provider = StatisticsProvider.of(
    EntityStatistics(TableStatistics(catalog.entity("customer").get.id, 1000, 40, 1, 0, 1000, true, Map.empty), 0),
    EntityStatistics(TableStatistics(catalog.entity("orders").get.id, 10, 24, 1, 0, 10, true, Map.empty), 0)
  )

  /** Plans `sql` with the cost-based policy (or `rule`) over `stats`. */
  private def planned(sql: String, stats: StatisticsProvider = provider, config: OptimizerConfig = OptimizerConfig.Default, rule: Boolean = false): PlannedQuery =
    val bound = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    val logical = new RuleOptimizer().optimize(LogicalPlanner.plan(bound))
    val estimator = new CardinalityEstimator(stats, catalog, config.estimation)
    val costModel = new CostModel(estimator, config)
    val policy = if rule then DefaultPlanningPolicy else new CostBasedPolicy(estimator, costModel)
    PhysicalPlanner.plan(logical, policy, Some(PlanEstimation(estimator, costModel)))

  /** The first hash join of a plan. */
  private def hashJoin(plan: PhysicalPlan): HashJoin = PhysicalPlan.preorder(plan).collectFirst { case j: HashJoin => j }.get

  /** Entity scanned directly below `plan`. */
  private def entity(plan: PhysicalPlan): Long = PhysicalPlan.preorder(plan).collectFirst { case EntityScan(e, _) => e.value }.get

  /** The smaller input is built: no swap when it is already on the right, a swap otherwise. */
  @Test def buildsSmallerInput(): Unit =
    val customer = catalog.entity("customer").get.id.value
    val orders = catalog.entity("orders").get.id.value
    val natural = planned("SELECT c.name FROM customer c JOIN orders o ON c.id = o.customer_id")
    assertEquals(orders, entity(hashJoin(natural.plan).right))
    assertFalse(natural.decisions.head.reason.contains("swapped"), natural.decisions.head.reason)
    val swapped = planned("SELECT c.name FROM orders o JOIN customer c ON c.id = o.customer_id")
    val join = hashJoin(swapped.plan)
    assertEquals(orders, entity(join.right), Explain.physical(swapped.plan, swapped.slotNames))
    assertEquals(customer, entity(join.left))
    val rightSlots = PhysicalPlan.preorder(join.right).collectFirst { case EntityScan(_, cs) => cs.map(_.slot).toSet }.get
    assertTrue(join.keys.forall(k => rightSlots(k.right)), "keys follow the swapped inputs")
    assertTrue(swapped.decisions.head.reason.contains("inputs swapped"), swapped.decisions.head.reason)

  /** LEFT JOIN keeps its right input as the build side, however large. */
  @Test def leftJoinNeverSwaps(): Unit =
    val q = planned("SELECT o.id FROM orders o LEFT JOIN customer c ON c.id = o.customer_id")
    val join = hashJoin(q.plan)
    assertEquals(PhysicalJoinType.Left, join.joinType)
    assertEquals(catalog.entity("customer").get.id.value, entity(join.right))

  /** Every node is annotated by pre-order id; the join and TopK carry their estimates. */
  @Test def annotatesEveryNode(): Unit =
    val q = planned("SELECT c.name, o.amount FROM customer c JOIN orders o ON c.id = o.customer_id ORDER BY o.amount DESC LIMIT 5")
    val nodes = PhysicalPlan.preorder(q.plan)
    assertEquals(nodes.indices.toSet, q.estimates.keySet, Explain.physical(q.plan, q.slotNames))
    val topK = nodes.indexWhere(_.isInstanceOf[TopK])
    assertEquals(5.0, q.estimates(topK).rows)
    val join = nodes.indexWhere(_.isInstanceOf[HashJoin])
    assertEquals(10.0, q.estimates(join).rows, 1e-9, "each order matches one customer at most (primary-key side)")
    assertTrue(q.estimates(0).cost >= q.estimates(join).cost, "costs are cumulative")
    assertEquals("statistics", q.estimates(join).source)
    assertTrue(q.warnings.isEmpty, q.warnings.toString)
    assertTrue(q.decisions.exists(d => d.choice == "top_k" && d.reason.contains("keeps 5 of ~10 rows")), q.decisions.toString)

  /** Missing or stale statistics become warnings naming the ANALYZE to run. */
  @Test def warnsAboutStatistics(): Unit =
    val missing = planned("SELECT c.name FROM customer c JOIN orders o ON c.id = o.customer_id", StatisticsProvider.Empty)
    assertEquals(2, missing.warnings.size)
    assertTrue(missing.warnings.forall(_.contains("run ANALYZE")), missing.warnings.toString)
    val staleOrders = StatisticsProvider.of(
      provider.statistics(catalog.entity("customer").get.id).get,
      provider.statistics(catalog.entity("orders").get.id).get.copy(modificationsSinceAnalyze = 5)
    )
    val stale = planned("SELECT o.id FROM orders o", staleOrders)
    assertEquals(Vector("statistics of orders are stale (5 modifications since ANALYZE, 50% of its rows); run ANALYZE orders"), stale.warnings)

  /** When every strategy exceeds an engine limit the cheapest is chosen and a warning says why. */
  @Test def warnsWhenNoStrategyFits(): Unit =
    val tight = OptimizerConfig(limits = EngineLimits(maxNestedLoopComparisons = 100))
    val q = planned("SELECT c.name FROM customer c JOIN orders o ON o.amount > c.id", config = tight)
    assertTrue(PhysicalPlan.preorder(q.plan).exists(_.isInstanceOf[NestedLoopJoin]))
    assertTrue(q.warnings.exists(_.contains("no strategy fits the engine limits")), q.warnings.toString)

  /** The rule policy still gets annotations but keeps the SQL build side. */
  @Test def rulePolicyIsAnnotatedButUnchanged(): Unit =
    val q = planned("SELECT c.name FROM orders o JOIN customer c ON c.id = o.customer_id", rule = true)
    assertEquals(catalog.entity("customer").get.id.value, entity(hashJoin(q.plan).right))
    assertEquals(PhysicalPlan.preorder(q.plan).indices.toSet, q.estimates.keySet)
