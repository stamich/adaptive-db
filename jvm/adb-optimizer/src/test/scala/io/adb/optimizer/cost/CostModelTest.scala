package io.adb.optimizer.cost

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*
import io.adb.optimizer.{EngineLimits, OptimizerConfig, RuleOptimizer}
import io.adb.optimizer.cardinality.CardinalityEstimator
import io.adb.sql.SqlParser
import io.adb.statistics.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Cost formulas, build-side choice, limit checks and bounded arithmetic. */
final class CostModelTest:
  /** small(id, v) with 10 rows and big(id, small_id, v) with 100,000 rows. */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("small", Vector(("id", DataType.Int64, false), ("v", DataType.Int64, false)), "id")
    c.createEntity("big", Vector(("id", DataType.Int64, false), ("small_id", DataType.Int64, false), ("v", DataType.Int64, false)), "id")
    c
  /** Row-count-only statistics of both tables. */
  private val provider = StatisticsProvider.of(
    EntityStatistics(TableStatistics(catalog.entity("small").get.id, 10, 20, 1, 0, 10, true, Map.empty), 0),
    EntityStatistics(TableStatistics(catalog.entity("big").get.id, 100_000, 30, 1, 0, 30_000, false, Map.empty), 0)
  )

  /** Model over the fixture with `config`. */
  private def model(config: OptimizerConfig = OptimizerConfig.Default): CostModel =
    new CostModel(new CardinalityEstimator(provider, catalog), config)

  /** The optimized logical plan of `sql`. */
  private def plan(sql: String): LogicalPlan =
    new RuleOptimizer().optimize(LogicalPlanner.plan(new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]))

  /** The first join of a plan. */
  private def join(plan: LogicalPlan): Join = plan match
    case j: Join => j
    case other => join(other.children.head)

  /** An INNER hash join builds the smaller input even when it is on the left. */
  @Test def innerJoinBuildsSmallerSide(): Unit =
    val m = model()
    val j = join(plan("SELECT s.v FROM small s JOIN big b ON b.small_id = s.id"))
    val chosen = m.joinCost(j.joinType, j.left, j.right, j.condition.toVector, new CardinalityEstimator(provider, catalog).estimate(j))
    assertEquals(BuildSide.Left, chosen.build, "small is the left input")
    assertTrue(chosen.violations.isEmpty)
    val rightBuild = m.strategyCost(hash = true, BuildSide.Right, j.left, j.right, 100_000)
    assertTrue(m.total(chosen.cost) < m.total(rightBuild.cost))

  /** A LEFT JOIN always builds the right input. */
  @Test def leftJoinBuildsRight(): Unit =
    val j = join(plan("SELECT s.v FROM small s LEFT JOIN big b ON b.small_id = s.id"))
    val chosen = model().joinCost(j.joinType, j.left, j.right, j.condition.toVector, new CardinalityEstimator(provider, catalog).estimate(j))
    assertEquals(BuildSide.Right, chosen.build)

  /** Exceeding the engine limits is reported, and a plan within them is preferred. */
  @Test def reportsLimitViolations(): Unit =
    val tight = OptimizerConfig(limits = EngineLimits(queryMemoryBytes = 1024, maxMaterializedRows = 50_000, maxNestedLoopComparisons = 1000))
    val m = model(tight)
    val j = join(plan("SELECT s.v FROM small s JOIN big b ON b.v > s.v"))
    val bigBuild = m.strategyCost(hash = false, BuildSide.Right, j.left, j.right, 1)
    assertEquals(3, bigBuild.violations.size, bigBuild.violations.toString)
    val chosen = m.joinCost(j.joinType, j.left, j.right, j.condition.toVector, new CardinalityEstimator(provider, catalog).estimate(j))
    assertEquals(BuildSide.Left, chosen.build)
    assertEquals(1, chosen.violations.size, "only the comparison limit remains")

  /** Cumulative costs add the inputs; a TopK costs less than a full sort of the same input. */
  @Test def cumulativeCostsAndTopK(): Unit =
    val m = model()
    val sorted = plan("SELECT b.v FROM big b ORDER BY b.v")
    val scan = sorted.children.head.children.head
    assertTrue(scan.isInstanceOf[TableScan], LogicalPlan.render(sorted))
    assertTrue(m.total(m.cost(sorted)) > m.total(m.cost(scan)))
    val sort = sorted.children.head
    assertTrue(m.total(m.topKCost(scan, 10)) < m.total(m.ownCost(sort)))
    assertEquals(100_000.0, m.cost(scan).cpu, 1e-9)

  /** Arithmetic saturates instead of overflowing; invalid weights are rejected. */
  @Test def boundedArithmetic(): Unit =
    val huge = Cost.bounded(Double.PositiveInfinity, Double.NaN, -1)
    assertEquals(Cost(Cost.Max, Cost.Max, 0), huge)
    assertEquals(Cost.Max, (huge + huge).total(CostWeights()))
    assertThrows(classOf[IllegalArgumentException], () => CostWeights(cpu = -1))
    assertThrows(classOf[IllegalArgumentException], () => OptimizerConfig(maxDpRelations = 20))

  /** A hash join beats a nested loop on large inputs; on tiny inputs the nested loop is cheaper. */
  @Test def hashVersusNestedLoop(): Unit =
    val m = model()
    val large = join(plan("SELECT s.v FROM big s JOIN big b ON b.small_id = s.id"))
    val hash = m.strategyCost(hash = true, BuildSide.Right, large.left, large.right, 100_000)
    val loop = m.strategyCost(hash = false, BuildSide.Right, large.left, large.right, 100_000)
    assertTrue(m.total(hash.cost) < m.total(loop.cost))
    val tinyProvider = StatisticsProvider.of(
      EntityStatistics(TableStatistics(catalog.entity("small").get.id, 2, 20, 1, 0, 2, true, Map.empty), 0))
    val tinyModel = new CostModel(new CardinalityEstimator(tinyProvider, catalog), OptimizerConfig.Default)
    val tiny = join(plan("SELECT a.v FROM small a JOIN small b ON a.id = b.id"))
    assertTrue(
      tinyModel.total(tinyModel.strategyCost(hash = false, BuildSide.Right, tiny.left, tiny.right, 2).cost) <
        tinyModel.total(tinyModel.strategyCost(hash = true, BuildSide.Right, tiny.left, tiny.right, 2).cost))

  /** A hash join's work is its hashed rows (build plus probe), weighted by the fitted hash-row weight. */
  @Test def hashJoinCountsHashedRows(): Unit =
    val m = model()
    val j = join(plan("SELECT s.v FROM big s JOIN big b ON b.small_id = s.id"))
    val hash = m.strategyCost(hash = true, BuildSide.Right, j.left, j.right, 50_000)
    assertEquals(200_000.0, hash.cost.hashRows, 1e-9)
    assertEquals(25_000.0, hash.cost.cpu, 1e-9)
    val weights = CostWeights()
    assertEquals(25_000.0 + 200_000.0 * weights.hashRow + hash.cost.memoryBytes * weights.memory, m.total(hash.cost), 1e-6)
    val doubled = new CostModel(new CardinalityEstimator(provider, catalog), OptimizerConfig(weights = weights.copy(hashRow = 2 * weights.hashRow)))
    assertTrue(doubled.total(hash.cost) > m.total(hash.cost))

  /** Index access costs: covering scans skip the fetch, locality makes fetches sequential, and an
    * ordered scan that stops early costs its fraction plus the full descent.
    */
  @Test def indexAccessCosts(): Unit =
    import io.adb.catalog.{IndexInfo, IndexKeyColumn}
    import io.adb.optimizer.access.{AccessPath, IndexPredicateMatcher}
    val bound = new Binder(catalog).bind(new SqlParser().parse("SELECT b.v FROM big b WHERE b.small_id = 3")).asInstanceOf[BoundSelect]
    val relation = bound.base
    val conjuncts = TypedExpr.conjuncts(bound.predicate.get)
    val big = catalog.entity("big").get.id
    val index = IndexInfo(IndexId(1), "i", big, Vector(IndexKeyColumn(FieldId(2), false)), Vector(FieldId(3)), 0, 0, 1)
    val matched = IndexPredicateMatcher.matchIndex(relation, index, conjuncts)
    val m = model()
    val covering = m.accessCost(AccessPath.IndexScan(relation, matched, backward = false, covering = true))
    val fetching = m.accessCost(AccessPath.IndexScan(relation, matched, backward = false, covering = false))
    val seq = m.accessCost(AccessPath.SeqScan(relation, conjuncts))
    assertTrue(m.total(covering.cost) < m.total(fetching.cost))
    assertTrue(m.total(fetching.cost) < m.total(seq.cost), "1 of 10 values: the index wins")
    assertEquals(fetching.read, fetching.cost.randomIo - 2.0, 1e-9, "one random fetch per matched row plus a descent of 2")
    val local = new CostModel(new CardinalityEstimator(StatisticsProvider.of(
      EntityStatistics(TableStatistics(big, 100_000, 30, 1, 0, 30_000, false, Map.empty,
        Map(IndexId(1) -> IndexStatistics(IndexId(1), 100_000, 100_000, 3, 500, 9, Vector(10), 1.0))), 0)), catalog), OptimizerConfig.Default)
    val sequentialFetch = local.accessCost(AccessPath.IndexScan(relation, matched, backward = false, covering = false))
    assertEquals(3.0, sequentialFetch.cost.randomIo, 1e-9, "fetches in storage order are not random")
    val half = m.accessCost(AccessPath.IndexScan(relation, matched, backward = false, covering = true), fraction = 0.5)
    assertTrue(m.total(half.cost) < m.total(covering.cost))
    assertEquals(covering.cost.randomIo, half.cost.randomIo, 1e-9, "the descent is paid in full")
