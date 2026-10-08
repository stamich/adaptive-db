package io.adb.optimizer.join

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*
import io.adb.optimizer.{OptimizerConfig, RuleOptimizer}
import io.adb.optimizer.cardinality.CardinalityEstimator
import io.adb.optimizer.cost.CostModel
import io.adb.sql.SqlParser
import io.adb.statistics.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Join ordering: exact, greedy and SQL-order strategies; boundaries; preserved semantics. */
final class JoinReorderRuleTest:
  /** fact(20,000 rows) -> dim(2,000) -> tiny(20); fact.dim_id and dim.tiny_id are foreign keys. */
  private val catalog =
    val c = new InMemoryCatalog
    val tiny = c.createEntity("tiny", Vector(("id", DataType.Int64, false), ("tag", DataType.StringType, false)), "id")
    val dim = c.createEntity("dim", Vector(("id", DataType.Int64, false), ("tiny_id", DataType.Int64, false)), "id",
      Map("tiny_id" -> ForeignKeyRef(tiny.id, tiny.primaryKey)))
    c.createEntity("fact", Vector(("id", DataType.Int64, false), ("dim_id", DataType.Int64, false), ("v", DataType.Int64, false)), "id",
      Map("dim_id" -> ForeignKeyRef(dim.id, dim.primaryKey)))
    c
  /** Statistics: row counts plus a 20-value tag column on tiny. */
  private val provider =
    def table(name: String, rows: Long, columns: Map[FieldId, ColumnStatistics] = Map.empty) =
      EntityStatistics(TableStatistics(catalog.entity(name).get.id, rows, 24, 1, 0, rows, true, columns), 0)
    StatisticsProvider.of(
      table("fact", 20_000),
      table("dim", 2_000),
      table("tiny", 20, Map(FieldId(2) -> ColumnStatistics(FieldId(2), 0, 20, true, None, None, 4, Vector.empty, Vector.empty)))
    )

  /** The rule-optimized plan of `sql`. */
  private def optimized(sql: String): LogicalPlan =
    new RuleOptimizer().optimize(LogicalPlanner.plan(new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]))

  /** Reorders `plan` with `config`. */
  private def reorder(plan: LogicalPlan, config: OptimizerConfig = OptimizerConfig.Default): (LogicalPlan, Vector[String]) =
    val estimator = new CardinalityEstimator(provider, catalog, config.estimation)
    new JoinReorderRule(estimator, new CostModel(estimator, config)).reorder(plan)

  /** Every join of `plan`, top-down. */
  private def joins(plan: LogicalPlan): Vector[Join] = plan match
    case join: Join => join +: (joinsOf(join.left) ++ joinsOf(join.right))
    case other => other.children.flatMap(joins)
  /** Joins below one input. */
  private def joinsOf(plan: LogicalPlan): Vector[Join] = joins(plan)

  /** Aliases read by `plan`. */
  private def aliases(plan: LogicalPlan): Set[String] = plan match
    case TableScan(r) => Set(r.alias)
    case PointLookup(r, _) => Set(r.alias)
    case other => other.children.flatMap(aliases).toSet

  /** Every conjunct of every join condition and filter of `plan`. */
  private def conjuncts(plan: LogicalPlan): Set[TypedExpr] =
    val own = plan match
      case Join(_, _, _, condition) => condition.toVector.flatMap(TypedExpr.conjuncts)
      case Filter(_, predicate) => TypedExpr.conjuncts(predicate)
      case _ => Vector.empty
    own.toSet ++ plan.children.flatMap(conjuncts)

  /** The star query of the join-order benchmark. */
  private val StarQuery =
    "SELECT f.v, t.tag FROM fact f JOIN dim d ON f.dim_id = d.id JOIN tiny t ON d.tiny_id = t.id WHERE t.tag = 'x'"

  /** The selective dimension pair joins first; the large fact table joins last. */
  @Test def joinsSelectiveRelationsFirst(): Unit =
    val original = optimized(StarQuery)
    val (plan, notes) = reorder(original)
    val top = joins(plan).head
    val innermost = joins(plan).last
    assertEquals(Set("d", "t"), aliases(innermost), LogicalPlan.render(plan))
    assertEquals(Set("f", "d", "t"), aliases(top))
    assertEquals(conjuncts(original), conjuncts(plan), "no condition lost or duplicated")
    assertEquals(original.output, plan.output, "the projection keeps the result columns")
    assertTrue(notes.head.startsWith("join order ((d JOIN t) JOIN f)") || notes.head.startsWith("join order (f JOIN (d JOIN t))"), notes.head)
    assertTrue(notes.head.contains("dynamic programming over 3 relations"), notes.head)

  /** Greedy ordering beyond the DP threshold finds the same order; the SQL order is kept beyond
    * the greedy threshold.
    */
  @Test def greedyAndSqlOrder(): Unit =
    val original = optimized(StarQuery)
    val (greedy, greedyNotes) = reorder(original, OptimizerConfig(maxDpRelations = 2, maxGreedyRelations = 3))
    assertEquals(Set("d", "t"), aliases(joins(greedy).last))
    assertTrue(greedyNotes.head.contains("greedy ordering of 3 relations"), greedyNotes.head)
    val (kept, keptNotes) = reorder(original, OptimizerConfig(maxDpRelations = 2, maxGreedyRelations = 2))
    assertEquals(Set("f", "d"), aliases(joins(kept).last))
    assertTrue(keptNotes.head.contains("SQL order kept"), keptNotes.head)

  /** A disconnected block still joins everything, with a cross product. */
  @Test def disconnectedBlockUsesCrossProduct(): Unit =
    val (plan, _) = reorder(optimized("SELECT f.v FROM fact f CROSS JOIN tiny t JOIN dim d ON f.dim_id = d.id"))
    assertEquals(1, joins(plan).count(_.joinType == JoinType.Cross), LogicalPlan.render(plan))
    assertEquals(Set("f", "d", "t"), aliases(joins(plan).head))

  /** LEFT JOIN is a boundary: its right input stays its right input, the inner block below it is
    * reordered on its own.
    */
  @Test def leftJoinIsABoundary(): Unit =
    val original = optimized(
      "SELECT f.v FROM fact f JOIN dim d ON f.dim_id = d.id JOIN tiny t ON d.tiny_id = t.id LEFT JOIN tiny t2 ON t2.id = d.tiny_id WHERE t.tag = 'x'"
    )
    val (plan, notes) = reorder(original)
    val top = joins(plan).head
    assertEquals(JoinType.Left, top.joinType)
    assertEquals(Set("t2"), aliases(top.right))
    assertEquals(Set("d", "t"), aliases(joins(top.left).last))
    assertEquals(1, notes.size)
    assertEquals(conjuncts(original), conjuncts(plan))

  /** A query without joins is unchanged and produces no notes. */
  @Test def singleRelationUnchanged(): Unit =
    val original = optimized("SELECT v FROM fact WHERE v > 3")
    assertEquals((original, Vector.empty), reorder(original))
