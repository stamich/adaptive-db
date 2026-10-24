package io.adb.physical

import io.adb.catalog.{IndexCatalog, IndexInfo, IndexKeyColumn, InMemoryCatalog}
import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.{OptimizerConfig, RuleOptimizer}
import io.adb.optimizer.cardinality.CardinalityEstimator
import io.adb.optimizer.cost.CostModel
import io.adb.physical.PhysicalPlan.*
import io.adb.sql.SqlParser
import io.adb.statistics.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Cost-based access paths: index scans versus sequential scans, covering scans, residual
  * placement, snapshots an index cannot serve, and rule mode.
  */
final class AccessPathPlanningTest:
  /** account(id, region, balance, owner, note): 100,000 rows; balance 0..9,999 uniform. */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("account", Vector(("id", DataType.Int64, false), ("region", DataType.Int64, false), ("balance", DataType.Int64, false),
      ("owner", DataType.StringType, false), ("note", DataType.StringType, true)), "id")
    c.createEntity("other", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false)), "id")
    c
  /** Entity and field ids. */
  private val account = catalog.entity("account").get.id
  private val (region, balance, owner) = (FieldId(2), FieldId(3), FieldId(4))
  /** idx_balance (balance) INCLUDE (owner); idx_region_balance (region, balance). */
  private val indexes = IndexCatalog.of(
    IndexInfo(IndexId(1), "idx_balance", account, Vector(IndexKeyColumn(balance, false)), Vector(owner), 50, 50, 100),
    IndexInfo(IndexId(2), "idx_region_balance", account, Vector(IndexKeyColumn(region, false), IndexKeyColumn(balance, false)), Vector.empty, 50, 50, 100),
    IndexInfo(IndexId(3), "idx_other", catalog.entity("other").get.id, Vector(IndexKeyColumn(FieldId(2), false)), Vector.empty, 50, 50, 100)
  )
  /** Analyzed statistics: balance uniform over 0..9,999, 10 regions. */
  private val statistics = StatisticsProvider.of(EntityStatistics(TableStatistics(account, 100_000, 60, 90, 0, 100_000, true, Map(
    balance -> ColumnStatistics(balance, 0, 10_000, true, Some(DbValue.Int64Value(0)), Some(DbValue.Int64Value(9_999)), 8,
      (0 until 10).map(b => HistogramBucket(DbValue.Int64Value(b * 1000), DbValue.Int64Value(b * 1000 + 999), 10_000, 1000)).toVector, Vector.empty),
    region -> ColumnStatistics(region, 0, 10, true, Some(DbValue.Int64Value(0)), Some(DbValue.Int64Value(9)), 8, Vector.empty, Vector.empty)
  )), 0))

  /** Plans `sql` in cost mode (or rule mode). */
  private def planned(sql: String, rule: Boolean = false): PlannedQuery =
    val bound = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    val logical = new RuleOptimizer().optimize(LogicalPlanner.plan(bound))
    val estimator = new CardinalityEstimator(statistics, catalog)
    val costModel = new CostModel(estimator, OptimizerConfig.Default)
    val policy = if rule then DefaultPlanningPolicy else new CostBasedPolicy(estimator, costModel, indexes, bound.asOfVersion)
    PhysicalPlanner.plan(logical, policy, Some(PlanEstimation(estimator, costModel)))

  /** The nodes of a plan in pre-order. */
  private def nodes(q: PlannedQuery): Vector[PhysicalPlan] = PhysicalPlan.preorder(q.plan)

  /** A selective equality on a covered query uses the index without fetching rows. */
  @Test def selectiveCoveredEqualityUsesTheIndex(): Unit =
    val q = planned("SELECT owner FROM account WHERE balance = 5000")
    val scan = nodes(q).collectFirst { case s: IndexScan => s }.getOrElse(fail(Explain.physical(q.plan, q.slotNames)))
    assertEquals(IndexId(1), scan.indexId)
    assertEquals(IndexBounds(Vector(DbValue.Int64Value(5000)), None, None), scan.bounds)
    assertFalse(nodes(q).exists(_.isInstanceOf[RowFetch]), "covering")
    assertTrue(q.decisions.exists(d => d.operator == "access account" && d.choice.contains("idx_balance")), q.decisions.toString)
    assertTrue(q.estimates.values.exists(_.components.randomIo > 0))

  /** A selective range fetches the remaining columns; residual conjuncts on covered columns are
    * applied before the fetch, others after it.
    */
  @Test def selectiveRangeFetchesRowsAndPlacesResiduals(): Unit =
    val q = planned("SELECT id, note FROM account WHERE balance >= 9990 AND owner <> 'x' AND note <> 'y'")
    val shape = nodes(q).map(_.getClass.getSimpleName)
    assertEquals(Vector("Project", "Filter", "RowFetch", "Filter", "IndexScan"), shape, Explain.physical(q.plan, q.slotNames))
    val scan = nodes(q).collectFirst { case s: IndexScan => s }.get
    assertEquals(Some(ValueBound(DbValue.Int64Value(9990), true)), scan.bounds.lower)

  /** An unselective predicate keeps the sequential scan; an index of another entity is never used. */
  @Test def unselectivePredicatesScanTheTable(): Unit =
    val q = planned("SELECT id, note FROM account WHERE balance >= 100")
    assertTrue(nodes(q).exists(_.isInstanceOf[EntityScan]), Explain.physical(q.plan, q.slotNames))
    assertFalse(nodes(q).exists(_.isInstanceOf[IndexScan]))
    assertTrue(q.decisions.exists(d => d.choice == "entity_scan" && d.reason.contains("next best index_scan")))

  /** Equality prefix plus range on a composite index. */
  @Test def compositePrefixAndRange(): Unit =
    val q = planned("SELECT id FROM account WHERE region = 4 AND balance < 20")
    val scan = nodes(q).collectFirst { case s: IndexScan => s }.getOrElse(fail(Explain.physical(q.plan, q.slotNames)))
    assertEquals(IndexId(2), scan.indexId)
    assertEquals(Vector(DbValue.Int64Value(4)), scan.bounds.prefix)
    assertEquals(Some(ValueBound(DbValue.Int64Value(20), false)), scan.bounds.upper)

  /** Snapshots before an index was built, and rule mode, never use it. */
  @Test def unusableSnapshotsAndRuleModeScanTheTable(): Unit =
    val old = planned("SELECT owner FROM account AS OF VERSION 10 WHERE balance = 5000")
    assertFalse(nodes(old).exists(_.isInstanceOf[IndexScan]), Explain.physical(old.plan, old.slotNames))
    val recent = planned("SELECT owner FROM account AS OF VERSION 60 WHERE balance = 5000")
    assertTrue(nodes(recent).exists(_.isInstanceOf[IndexScan]))
    val rule = planned("SELECT owner FROM account WHERE balance = 5000", rule = true)
    assertFalse(nodes(rule).exists(_.isInstanceOf[IndexScan]))
    assertTrue(rule.decisions.forall(!_.operator.startsWith("access")))
