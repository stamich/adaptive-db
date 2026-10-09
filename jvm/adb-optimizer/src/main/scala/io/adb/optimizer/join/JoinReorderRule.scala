package io.adb.optimizer.join

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.optimizer.{OptimizerConfig, Rule}
import io.adb.optimizer.cardinality.{CardinalityEstimator, Estimate}
import io.adb.optimizer.cost.CostModel

/** What join ordering decided for one join block (shown by EXPLAIN).
  *
  * @param order  the chosen tree, e.g. `((d JOIN t) JOIN f)`
  * @param method how it was found
  * @param rows   estimated rows of the block
  * @param cost   weighted cumulative cost of the block
  */
final case class JoinOrderNote(order: String, method: String, rows: Double, cost: Double) derives CanEqual:
  /** `join order ((d JOIN t) JOIN f): dynamic programming over 3 relations, est. rows=… cost=…`. */
  def display: String = f"join order $order: $method, est. rows=$rows%.0f cost=$cost%.1f"

/** Chooses the order of INNER and CROSS joins by estimated cost.
  *
  * Every maximal block of INNER/CROSS joins (see [[JoinGraph]]) is reordered independently:
  *
  *  - up to [[OptimizerConfig.maxDpRelations]] relations: dynamic programming over relation
  *    subsets (bushy trees included), considering only splits joined by a predicate unless no
  *    such split exists (then the block is disconnected and a cross product is unavoidable);
  *  - up to [[OptimizerConfig.maxGreedyRelations]]: greedy pairing, repeatedly joining the two
  *    connected parts with the smallest estimated result;
  *  - beyond that the SQL order is kept.
  *
  * Each join's cost is the cheaper strategy and build side of [[CostModel.joinCost]]; a join
  * whose estimates exceed an engine limit gets a large penalty, so a plan the engine can run
  * always wins over one it would abort. Conditions are attached to the lowest join that sees
  * all the relations they read. The rule runs after [[io.adb.optimizer.RuleOptimizer]], so
  * single-relation filters already sit on their scans.
  *
  * Output attribute order changes with the join order; every operator above a join addresses
  * columns by slot, so the query result does not.
  *
  * @param estimator cardinality estimates
  * @param costModel operator costs
  */
final class JoinReorderRule(estimator: CardinalityEstimator, costModel: CostModel) extends Rule:
  /** Rule name shown in optimizer traces. */
  val name = "JoinReorderRule"
  /** Added to the cost of a join whose estimates exceed an engine limit. */
  private val LimitPenalty = 1e15
  /** Configuration (thresholds). */
  private val config: OptimizerConfig = costModel.config

  /** One way to compute a set of relations.
    *
    * @param plan     the join tree
    * @param estimate its estimated output
    * @param cost     its weighted cumulative cost
    */
  private final case class Candidate(plan: LogicalPlan, estimate: Estimate, cost: Double)

  /** Reorders every join block of `plan`. */
  def apply(plan: LogicalPlan): LogicalPlan = reorder(plan)._1

  /** Reorders every join block of `plan` and describes what was decided for each block. */
  def reorder(plan: LogicalPlan): (LogicalPlan, Vector[JoinOrderNote]) =
    val notes = Vector.newBuilder[JoinOrderNote]
    /** Reorders the block rooted at `node`, or recurses into its inputs. */
    def rewrite(node: LogicalPlan): LogicalPlan = node match
      case join: Join if JoinGraph.isReorderable(join) =>
        val graph = JoinGraph.of(join, rewrite)
        val (ordered, note) = order(graph)
        notes += note
        ordered
      case other => mapChildren(other)(rewrite)
    val result = rewrite(plan)
    (result, notes.result())

  /** The cheapest join tree of `graph` and a note describing the method. */
  private def order(graph: JoinGraph): (LogicalPlan, JoinOrderNote) =
    val n = graph.relations.size
    val (best, method) =
      if n <= config.maxDpRelations then (dynamicProgramming(graph), s"dynamic programming over $n relations")
      else if n <= config.maxGreedyRelations then (greedy(graph), s"greedy ordering of $n relations")
      else (sqlOrder(graph), s"SQL order kept for $n relations (more than ${config.maxGreedyRelations})")
    val tree = withConstants(best.plan, graph.constant)
    (tree, JoinOrderNote(shape(tree), method, best.estimate.rows, best.cost))

  /** Exact search over all subsets: `best(S) = min over splits S1 ∪ S2 = S of best(S1) ⋈ best(S2)`. */
  private def dynamicProgramming(graph: JoinGraph): Candidate =
    val n = graph.relations.size
    val best = new Array[Candidate](1 << n)
    for i <- 0 until n do best(1 << i) = leaf(graph.relations(i))
    for subset <- (1 until (1 << n)).sortBy(Integer.bitCount) if Integer.bitCount(subset) >= 2 do
      val lowest = Integer.lowestOneBit(subset)
      val splits = Iterator.iterate((subset - 1) & subset)(s => (s - 1) & subset).takeWhile(_ != 0)
        .filter(s => (s & lowest) != 0).map(s => (s.toLong, (subset & ~s).toLong)).toVector
      val connected = splits.filter((a, b) => graph.connected(a, b))
      val usable = if connected.nonEmpty then connected else splits
      best(subset) = usable.map((a, b) => joined(graph, best(a.toInt), best(b.toInt), a, b)).reduce((x, y) => if y.cost < x.cost then y else x)
    best((1 << n) - 1)

  /** Greedy operator ordering: join the connected pair with the smallest result until one part remains. */
  private def greedy(graph: JoinGraph): Candidate =
    var parts = graph.relations.indices.map(i => (1L << i) -> leaf(graph.relations(i))).toVector
    while parts.size > 1 do
      val pairs = for i <- parts.indices; j <- parts.indices if i < j yield (i, j)
      val connected = pairs.filter((i, j) => graph.connected(parts(i)._1, parts(j)._1))
      val usable = if connected.nonEmpty then connected else pairs
      val (i, j, candidate) = usable
        .map((i, j) => (i, j, joined(graph, parts(i)._2, parts(j)._2, parts(i)._1, parts(j)._1)))
        .minBy((_, _, c) => (c.estimate.rows, c.cost))
      val merged = (parts(i)._1 | parts(j)._1) -> candidate
      parts = parts.zipWithIndex.collect { case (part, k) if k != i && k != j => part } :+ merged
    parts.head._2

  /** Left-deep join in SQL order with conditions at the lowest possible join. */
  private def sqlOrder(graph: JoinGraph): Candidate =
    graph.relations.indices.drop(1).foldLeft((1L, leaf(graph.relations.head))) { case ((mask, acc), i) =>
      val next = 1L << i
      (mask | next, joined(graph, acc, leaf(graph.relations(i)), mask, next))
    }._2

  /** A relation as a candidate. */
  private def leaf(plan: LogicalPlan): Candidate =
    Candidate(plan, estimator.estimate(plan), costModel.total(costModel.cost(plan)))

  /** `left ⋈ right` with every predicate that becomes evaluable. */
  private def joined(graph: JoinGraph, left: Candidate, right: Candidate, leftMask: Long, rightMask: Long): Candidate =
    val conditions = graph.joining(leftMask, rightMask)
    val joinType = if conditions.isEmpty then JoinType.Cross else JoinType.Inner
    val plan = Join(left.plan, right.plan, joinType, TypedExpr.conjunction(conditions))
    val estimate = estimator.join(joinType, left.estimate, right.estimate,
      left.plan.output.map(_.slot).toSet, right.plan.output.map(_.slot).toSet, conditions)
    val step = costModel.joinCost(joinType, left.plan, right.plan, conditions, estimate)
    val cost = left.cost + right.cost + costModel.total(step.cost) + step.violations.size * LimitPenalty
    Candidate(plan, estimate, cost)

  /** `plan` with conditions that read no relation added to its top join. */
  private def withConstants(plan: LogicalPlan, constants: Vector[TypedExpr]): LogicalPlan =
    if constants.isEmpty then plan
    else plan match
      case Join(left, right, _, condition) =>
        Join(left, right, JoinType.Inner, TypedExpr.conjunction(condition.toVector.flatMap(TypedExpr.conjuncts) ++ constants))
      case other => filtered(other, constants)

  /** `((a JOIN b) JOIN c)` rendering of a join tree by relation alias; a relation that is not
    * a single scan (a LEFT JOIN, an aggregate) shows the aliases it reads in brackets.
    */
  private def shape(plan: LogicalPlan): String = plan match
    case Join(left, right, _, _) if JoinGraph.isReorderable(plan) => s"(${shape(left)} JOIN ${shape(right)})"
    case other =>
      aliases(other) match
        case Vector(single) => single
        case several => several.mkString("[", ",", "]")

  /** Aliases of every relation read below `plan`. */
  private def aliases(plan: LogicalPlan): Vector[String] = plan match
    case TableScan(relation) => Vector(relation.alias)
    case PointLookup(relation, _) => Vector(relation.alias)
    case other => other.children.flatMap(aliases)
