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

  /** ORDER BY + LIMIT on an index key reads the index in order and stops: no TopK, no Sort;
    * descending order reads the index backward.
    */
  @Test def indexOrderReplacesTopK(): Unit =
    val q = planned("SELECT owner, balance FROM account WHERE balance >= 9000 ORDER BY balance LIMIT 10")
    assertFalse(nodes(q).exists(n => n.isInstanceOf[TopK] || n.isInstanceOf[Sort]), Explain.physical(q.plan, q.slotNames))
    assertTrue(nodes(q).exists(_.isInstanceOf[Limit]))
    val scan = nodes(q).collectFirst { case s: IndexScan => s }.get
    assertEquals(ScanDirection.Forward, scan.direction)
    assertEquals(Some(10), scan.limit)
    assertTrue(q.decisions.exists(_.choice == "access order + limit"), q.decisions.toString)
    val desc = planned("SELECT owner, balance FROM account ORDER BY balance DESC LIMIT 5")
    assertEquals(Some(ScanDirection.Backward), nodes(desc).collectFirst { case s: IndexScan => s.direction }, Explain.physical(desc.plan, desc.slotNames))
    assertFalse(nodes(desc).exists(_.isInstanceOf[TopK]))

  /** Primary-key order comes from the sequential scan; equality-bound key columns do not
    * disturb an index order; orders no path produces keep their TopK.
    */
  @Test def primaryKeyAndConstantColumns(): Unit =
    val byId = planned("SELECT id, note FROM account ORDER BY id LIMIT 3")
    assertEquals(Vector("Project", "Limit", "EntityScan"), nodes(byId).map(_.getClass.getSimpleName), Explain.physical(byId.plan, byId.slotNames))
    val constant = planned("SELECT id FROM account WHERE region = 2 ORDER BY region, balance LIMIT 5")
    assertFalse(nodes(constant).exists(_.isInstanceOf[TopK]), Explain.physical(constant.plan, constant.slotNames))
    assertEquals(Some(IndexId(2)), nodes(constant).collectFirst { case s: IndexScan => s.indexId })
    val other = planned("SELECT owner FROM account WHERE balance >= 9000 ORDER BY owner LIMIT 5")
    assertTrue(nodes(other).exists(_.isInstanceOf[TopK]), Explain.physical(other.plan, other.slotNames))
    val mixed = planned("SELECT id FROM account WHERE region = 2 ORDER BY balance DESC, id LIMIT 5")
    assertTrue(nodes(mixed).exists(_.isInstanceOf[TopK]), "mixed directions are not an index order")

  /** A full ORDER BY over a selective index range needs no Sort either. */
  @Test def indexOrderReplacesSort(): Unit =
    val q = planned("SELECT owner, balance FROM account WHERE balance >= 9990 ORDER BY balance")
    assertFalse(nodes(q).exists(_.isInstanceOf[Sort]), Explain.physical(q.plan, q.slotNames))
    assertTrue(q.decisions.exists(_.choice == "access order"))
    val rule = planned("SELECT owner, balance FROM account WHERE balance >= 9990 ORDER BY balance", rule = true)
    assertTrue(nodes(rule).exists(_.isInstanceOf[Sort]))

  /** EXPLAIN names indexes and bounds by column and marks covering scans; access decisions list
    * the rejected alternatives after the next best one.
    */
  @Test def explainNamesIndexesAndAlternatives(): Unit =
    val labels = Map(IndexId(1) -> IndexLabel("idx_balance", Vector("balance")), IndexId(2) -> IndexLabel("idx_region_balance", Vector("region", "balance")))
    val covered = planned("SELECT owner FROM account WHERE balance >= 9990")
    val text = Explain.physical(covered.plan, covered.slotNames, indexes = labels)
    assertTrue(text.contains("IndexScan idx_balance [balance >= 9990] covering"), text)
    val fetched = planned("SELECT note FROM account WHERE region = 3 AND balance > 9990 AND balance <= 9995")
    val fetchedText = Explain.physical(fetched.plan, fetched.slotNames, indexes = labels)
    assertTrue(fetchedText.contains("IndexScan idx_region_balance [region = 3, balance in (9990, 9995]] ["), fetchedText)
    assertFalse(fetchedText.contains("covering"), fetchedText)
    val ordered = planned("SELECT owner FROM account WHERE region = 2 ORDER BY balance LIMIT 5")
    val decision = ordered.decisions.find(_.choice.startsWith("index_scan")).getOrElse(fail(ordered.decisions.toString))
    assertTrue(decision.reason.contains("; next best ") && decision.reason.contains("; also rejected "), decision.reason)
    assertEquals("[region = 2, balance < 'x']", Explain.renderNamedBounds(
      IndexBounds(Vector(DbValue.Int64Value(2)), None, Some(ValueBound(DbValue.StringValue("x"), false))), Vector("region", "balance")))
    assertEquals("full", Explain.renderNamedBounds(IndexBounds(Vector.empty, None, None), Vector("balance")))
    assertEquals("[key0 = 'it''s']", Explain.renderNamedBounds(IndexBounds(Vector(DbValue.StringValue("it's")), None, None), Vector.empty))


  /** Every node of an index plan carries an estimate, including the row fetch and filters no
    * logical operator stands for (ordered LIMIT over a non-covering scan).
    */
  @Test def everyIndexPlanNodeHasAnEstimate(): Unit =
    for sql <- Vector(
        "SELECT note FROM account WHERE balance >= 9000 ORDER BY balance LIMIT 10",
        "SELECT note FROM account WHERE region = 3 AND balance > 9990 AND note = 'x'"
      )
    do
      val q = planned(sql)
      assertTrue(nodes(q).exists(_.isInstanceOf[RowFetch]), Explain.physical(q.plan, q.slotNames))
      assertEquals(nodes(q).indices.toSet, q.estimates.keySet, Explain.physical(q.plan, q.slotNames, q.estimates))
