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
import scala.util.Random

/** Properties of join ordering: optimality of the dynamic programming against exhaustive
  * enumeration, the golden order of the 2.2 plan, and unchanged query results (LEFT JOIN
  * included) on random data.
  */
final class JoinOrderPropertiesTest:
  /** Six tables r0..r5 (id, k, j) with nullable v. */
  private val catalog =
    val c = new InMemoryCatalog
    for i <- 0 until 6 do
      c.createEntity(s"r$i", Vector(("id", DataType.Int64, false), ("k", DataType.Int64, false), ("j", DataType.Int64, false), ("v", DataType.Int64, true)), "id")
    c

  /** The rule-optimized plan of `sql` over `cat`. */
  private def optimized(sql: String, cat: InMemoryCatalog = catalog): LogicalPlan =
    new RuleOptimizer().optimize(LogicalPlanner.plan(new Binder(cat).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]))

  /** Row-count-only statistics. */
  private def rowCounts(cat: InMemoryCatalog, rows: Map[String, Long]): StatisticsProvider =
    StatisticsProvider.of(rows.toSeq.map((name, n) =>
      EntityStatistics(TableStatistics(cat.entity(name).get.id, n, 32, 1, 0, n, true, Map.empty), 0))*)

  /** The topmost INNER/CROSS join of a plan. */
  private def block(plan: LogicalPlan): Join = plan match
    case j: Join if JoinGraph.isReorderable(j) => j
    case other => other.children.iterator.map(c => scala.util.Try(block(c))).collectFirst { case scala.util.Success(j) => j }.get

  /** A random connected join query over `n` relations: a random spanning tree in the ON clauses
    * plus random extra equalities in WHERE (pushdown moves them into the joins).
    */
  private def randomQuery(random: Random, n: Int): String =
    val joins = (1 until n).map { i =>
      val parent = random.nextInt(i)
      s" JOIN r$i a$i ON a$parent.k = a$i.j"
    }.mkString
    val extra = (0 until random.nextInt(3)).flatMap { _ =>
      val (x, y) = (random.nextInt(n), random.nextInt(n))
      Option.when(x != y)(s"a$x.j = a$y.k")
    }
    s"SELECT a0.id FROM r0 a0$joins" + (if extra.isEmpty then "" else extra.mkString(" WHERE ", " AND ", ""))

  /** Weighted cost of a join tree under the rule's own accounting. */
  private def treeCost(plan: LogicalPlan, estimator: CardinalityEstimator, model: CostModel): Double = plan match
    case join @ Join(left, right, joinType, condition) if JoinGraph.isReorderable(join) =>
      val conjuncts = condition.toVector.flatMap(TypedExpr.conjuncts)
      val out = estimator.join(joinType, estimator.estimate(left), estimator.estimate(right),
        left.output.map(_.slot).toSet, right.output.map(_.slot).toSet, conjuncts)
      val step = model.joinCost(joinType, left, right, conjuncts, out)
      treeCost(left, estimator, model) + treeCost(right, estimator, model) + model.total(step.cost) + step.violations.size * 1e15
    case leaf => model.total(model.cost(leaf))

  /** Every tree of `graph` over the connected relation set `mask` without cross products (both
    * halves connected and linked by a predicate), enumerated without memoization.
    */
  private def exhaustive(graph: JoinGraph, mask: Long, estimator: CardinalityEstimator, model: CostModel): Vector[LogicalPlan] =
    if java.lang.Long.bitCount(mask) == 1 then Vector(graph.relations(java.lang.Long.numberOfTrailingZeros(mask)))
    else
      val lowest = java.lang.Long.lowestOneBit(mask)
      val splits = Iterator.iterate((mask - 1) & mask)(s => (s - 1) & mask).takeWhile(_ != 0).filter(s => (s & lowest) != 0)
        .map(s => (s, mask & ~s)).toVector
      val connected = splits.filter((a, b) => graph.isConnected(a) && graph.isConnected(b) && graph.connected(a, b))
      for
        (a, b) <- connected
        left <- exhaustive(graph, a, estimator, model)
        right <- exhaustive(graph, b, estimator, model)
      yield
        val conditions = graph.joining(a, b)
        Join(left, right, if conditions.isEmpty then JoinType.Cross else JoinType.Inner, TypedExpr.conjunction(conditions))

  /** On random connected graphs of 3..6 relations, dynamic programming finds the exhaustive minimum. */
  @Test def dynamicProgrammingIsOptimal(): Unit =
    val random = new Random(2203)
    for round <- 0 until 40 do
      val n = 3 + round % 4
      val sql = randomQuery(random, n)
      // At least 1,000 rows per table and default distinct counts keep estimates independent of
      // the tree shape, so the optimum is well defined.
      val stats = rowCounts(catalog, (0 until n).map(i => s"r$i" -> (1000L + random.nextInt(100_000))).toMap)
      val estimator = new CardinalityEstimator(stats, catalog)
      val model = new CostModel(estimator, OptimizerConfig.Default)
      val plan = optimized(sql)
      val (reordered, _) = new JoinReorderRule(estimator, model).reorder(plan)
      val graph = JoinGraph.of(block(plan), identity)
      val best = exhaustive(graph, graph.all, estimator, model).map(treeCost(_, estimator, model)).min
      val chosen = treeCost(block(reordered), estimator, model)
      assertEquals(best, chosen, best * 1e-9, s"$sql\n${LogicalPlan.render(reordered)}")

  /** The 2.2 plan's golden case: A = 1,000,000 → B = 10,000 → C = 100 joins B with C first. */
  @Test def goldenThreeTableOrder(): Unit =
    val cat = new InMemoryCatalog
    val c = cat.createEntity("c", Vector(("id", DataType.Int64, false)), "id")
    val b = cat.createEntity("b", Vector(("id", DataType.Int64, false), ("c_id", DataType.Int64, false)), "id",
      Map("c_id" -> ForeignKeyRef(c.id, c.primaryKey)))
    cat.createEntity("a", Vector(("id", DataType.Int64, false), ("b_id", DataType.Int64, false)), "id",
      Map("b_id" -> ForeignKeyRef(b.id, b.primaryKey)))
    val estimator = new CardinalityEstimator(rowCounts(cat, Map("a" -> 1_000_000L, "b" -> 10_000L, "c" -> 100L)), cat)
    val plan = optimized("SELECT a.id FROM a JOIN b ON a.b_id = b.id JOIN c ON b.c_id = c.id", cat)
    val (reordered, notes) = new JoinReorderRule(estimator, new CostModel(estimator, OptimizerConfig.Default)).reorder(plan)
    assertTrue(Set("(a JOIN (b JOIN c))", "((b JOIN c) JOIN a)").contains(notes.head.order), notes.head.display)
    assertEquals(1_000_000.0, estimator.estimate(block(reordered)).rows, 1e-6)

  /** Reordering never changes results: random data, random INNER/LEFT/CROSS queries, evaluated
    * by a reference interpreter before and after reordering.
    */
  @Test def reorderingPreservesResults(): Unit =
    val random = new Random(7)
    val data = (0 until 6).map { i =>
      catalog.entity(s"r$i").get.id -> (1 to 4 + random.nextInt(6)).toVector.map { id =>
        Map(FieldId(1) -> DbValue.Int64Value(id), FieldId(2) -> DbValue.Int64Value(random.nextInt(4)),
          FieldId(3) -> DbValue.Int64Value(random.nextInt(4)),
          FieldId(4) -> (if random.nextInt(4) == 0 then DbValue.NullValue else DbValue.Int64Value(random.nextInt(5))))
      }
    }.toMap
    for _ <- 0 until 60 do
      val n = 2 + random.nextInt(4)
      val joins = (1 until n).map { i =>
        val parent = random.nextInt(i)
        random.nextInt(5) match
          case 0 => s" LEFT JOIN r$i a$i ON a$parent.k = a$i.j"
          case 1 => s" CROSS JOIN r$i a$i"
          case _ => s" JOIN r$i a$i ON a$parent.k = a$i.j" + (if random.nextBoolean() then s" AND a$i.v < 3" else "")
      }.mkString
      val where = if random.nextBoolean() then s" WHERE a${random.nextInt(n)}.v >= 1" else ""
      val sql = s"SELECT ${(0 until n).map(i => s"a$i.id, a$i.v").mkString(", ")} FROM r0 a0$joins$where"
      val plan = optimized(sql)
      val stats = rowCounts(catalog, (0 until 6).map(i => s"r$i" -> (1L + random.nextInt(50_000))).toMap)
      val estimator = new CardinalityEstimator(stats, catalog)
      val (reordered, _) = new JoinReorderRule(estimator, new CostModel(estimator, OptimizerConfig.Default)).reorder(plan)
      assertEquals(Reference.run(plan, data), Reference.run(reordered, data), s"$sql\n${LogicalPlan.render(reordered)}")

/** Minimal reference interpreter of logical plans (scans, filters, joins, projections). */
private object Reference:
  /** One row: slot to value. */
  type Row = Map[SlotId, DbValue]

  /** The sorted rendering of `plan`'s result over `data` (rows by entity, values by field). */
  def run(plan: LogicalPlan, data: Map[EntityId, Vector[Map[FieldId, DbValue]]]): Vector[String] =
    eval(plan, data).map(row => plan.output.map(a => row(a.slot).toString).mkString("|")).sorted

  /** Rows produced by `plan`. */
  private def eval(plan: LogicalPlan, data: Map[EntityId, Vector[Map[FieldId, DbValue]]]): Vector[Row] = plan match
    case TableScan(relation) => scan(relation, data)
    case PointLookup(relation, key) => scan(relation, data).filter { row =>
        relation.columns.exists(a => a.origin == ColumnOrigin.Stored(relation.id, relation.entity.id, relation.entity.primaryKey) && row(a.slot) == DbValue.Int64Value(key))
      }
    case Filter(input, predicate) => eval(input, data).filter(row => truth(predicate, row))
    case Join(left, right, joinType, condition) =>
      val rights = eval(right, data)
      eval(left, data).flatMap { l =>
        val matches = rights.map(l ++ _).filter(row => condition.forall(truth(_, row)))
        if matches.isEmpty && joinType == JoinType.Left then Vector(l ++ right.output.map(_.slot -> DbValue.NullValue))
        else matches
      }
    case Project(input, attributes) => eval(input, data).map(row => attributes.map(a => a.slot -> row(a.slot)).toMap)
    case other => throw new IllegalArgumentException(s"unsupported in the reference interpreter: $other")

  /** Rows of one relation instance. */
  private def scan(relation: BoundRelation, data: Map[EntityId, Vector[Map[FieldId, DbValue]]]): Vector[Row] =
    data(relation.entity.id).map(stored => relation.columns.map { a =>
      a.origin match
        case ColumnOrigin.Stored(_, _, field) => a.slot -> stored.getOrElse(field, DbValue.NullValue)
        case ColumnOrigin.Computed(_) => a.slot -> DbValue.NullValue
    }.toMap)

  /** Whether `expr` is true for `row` (NULL counts as false, as in the engine). */
  private def truth(expr: TypedExpr, row: Row): Boolean = expr match
    case TypedExpr.Binary(l, BinaryOp.And, r, _) => truth(l, row) && truth(r, row)
    case TypedExpr.Binary(l, BinaryOp.Or, r, _) => truth(l, row) || truth(r, row)
    case TypedExpr.Not(inner) => !truth(inner, row)
    case TypedExpr.Literal(DbValue.BoolValue(v)) => v
    case TypedExpr.Binary(l, op, r, _) =>
      (value(l, row), value(r, row)) match
        case (DbValue.Int64Value(a), DbValue.Int64Value(b)) =>
          op match
            case BinaryOp.Eq => a == b
            case BinaryOp.Ne => a != b
            case BinaryOp.Lt => a < b
            case BinaryOp.Le => a <= b
            case BinaryOp.Gt => a > b
            case BinaryOp.Ge => a >= b
            case _ => false
        case _ => false
    case _ => false

  /** Value of a column or literal. */
  private def value(expr: TypedExpr, row: Row): DbValue = expr match
    case TypedExpr.Column(attribute) => row(attribute.slot)
    case TypedExpr.Literal(v) => v
    case _ => DbValue.NullValue
