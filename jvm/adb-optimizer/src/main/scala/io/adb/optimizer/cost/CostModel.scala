package io.adb.optimizer.cost

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*
import io.adb.optimizer.OptimizerConfig
import io.adb.optimizer.cardinality.{CardinalityEstimator, Estimate}

/** Which input a blocking join materializes. */
enum BuildSide derives CanEqual:
  /** The right input (the engine's native layout; required by LEFT JOIN). */
  case Right
  /** The left input: the planner swaps the inputs before sending the plan. */
  case Left

/** Cost of one join strategy for one pair of inputs.
  *
  * @param cost       the join's own cost (inputs excluded)
  * @param build      the side the join materializes
  * @param violations engine limits the estimates exceed (empty when the plan is expected to run)
  */
final case class JoinCost(cost: Cost, build: BuildSide, violations: Vector[String]) derives CanEqual

/** Estimates the work of logical operators, using the cardinality estimator for row counts and
  * column statistics for row widths.
  *
  * Per-operator formulas (rows are estimates; `w` is the width of a materialized row):
  *
  * | Operator | cpu | io | memory |
  * |---|---|---|---|
  * | scan | rows | `rows · w / 8 KiB` | – |
  * | point lookup | 1 | 1 | – |
  * | filter | `0.25 · rows_in · conjuncts` | – | – |
  * | hash join | `2 · build + probe + 0.5 · out` | – | `build · w` |
  * | nested-loop join | `0.5 · outer · inner + 0.5 · out` | – | `inner · w` |
  * | aggregate | `1.5 · rows_in + groups` | – | `groups · w` |
  * | sort | `rows · log2(rows)` | – | `rows · w` |
  * | top-k | `rows · (1 + log2(k))` | – | `k · w` |
  * | project, limit | `0.05 · rows` | – | – |
  *
  * @param estimator row counts and statistics
  * @param config    weights and engine limits
  */
final class CostModel(estimator: CardinalityEstimator, val config: OptimizerConfig):
  /** Bytes the engine charges per slot on top of the value (row vector entry). */
  private val SlotOverheadBytes = 16.0
  /** Page size used to turn scanned bytes into pages. */
  private val PageBytes = 8192.0
  /** Cumulative costs computed so far, by plan node identity. */
  private val memo = new java.util.IdentityHashMap[LogicalPlan, Cost]()

  /** Weighted total of `cost`. */
  def total(cost: Cost): Double = cost.total(config.weights)

  /** Estimated width of a row holding `attributes`. */
  def rowBytes(attributes: Vector[Attribute]): Double = attributes.iterator.map(width).sum

  /** Estimated width of one value of `attribute` including slot overhead. */
  def width(attribute: Attribute): Double =
    val measured = attribute.origin match
      case ColumnOrigin.Stored(_, entity, field) =>
        estimator.entityStatistics(entity).flatMap(_.table.column(field)).map(_.avgWidthBytes)
      case ColumnOrigin.Computed(_) => None
    measured.getOrElse(defaultWidth(attribute.dataType)) + SlotOverheadBytes

  /** Cumulative cost of `plan` (the operator and all of its inputs). Joins are costed with
    * the strategy and build side [[joinCost]] picks, so the figure matches what the
    * cost-based planning policy will choose.
    */
  def cost(plan: LogicalPlan): Cost =
    val known = memo.get(plan)
    if known != null then known
    else
      val computed = plan.children.foldLeft(ownCost(plan))((sum, child) => sum + cost(child))
      memo.put(plan, computed)
      computed

  /** Cost of one operator without its inputs. */
  def ownCost(plan: LogicalPlan): Cost =
    val out = estimator.estimate(plan).rows
    plan match
      case TableScan(relation) => Cost.bounded(out, out * rowBytes(relation.columns) / PageBytes, 0)
      case PointLookup(_, _) => Cost(1, 1, 0)
      case Filter(input, predicate) =>
        Cost.bounded(0.25 * estimator.estimate(input).rows * TypedExpr.conjuncts(predicate).size, 0, 0)
      case join: Join =>
        val conjuncts = join.condition.toVector.flatMap(TypedExpr.conjuncts)
        joinCost(join.joinType, join.left, join.right, conjuncts, estimator.estimate(join)).cost
      case Aggregate(input, _, _) =>
        Cost.bounded(1.5 * estimator.estimate(input).rows + out, 0, out * rowBytes(plan.output))
      case Sort(input, _) =>
        val rows = estimator.estimate(input).rows
        Cost.bounded(rows * log2(rows), 0, rows * rowBytes(input.output))
      case Project(input, _) => Cost.bounded(0.05 * estimator.estimate(input).rows, 0, 0)
      case Limit(input, _) => Cost.bounded(0.05 * out, 0, 0)

  /** Cost of `LIMIT k` over `ORDER BY` executed as a top-k over `input`. */
  def topKCost(input: LogicalPlan, limit: Int): Cost =
    val rows = estimator.estimate(input).rows
    val kept = math.min(limit.toDouble, rows)
    Cost.bounded(rows * (1 + log2(kept)), 0, kept * rowBytes(input.output))

  /** Cost of the cheapest strategy for joining `left` and `right` under `conjuncts`, whose
    * output is estimated as `out`: a hash join when an equality key exists, a nested-loop
    * join otherwise; an INNER join materializes the smaller side, a LEFT join always the
    * right one.
    */
  def joinCost(joinType: JoinType, left: LogicalPlan, right: LogicalPlan, conjuncts: Vector[TypedExpr], out: Estimate): JoinCost =
    val leftSlots = left.output.map(_.slot).toSet
    val rightSlots = right.output.map(_.slot).toSet
    val hasKey = conjuncts.exists(isEquiKey(_, leftSlots, rightSlots)) && joinType != JoinType.Cross
    val sides = if joinType == JoinType.Left then Vector(BuildSide.Right) else Vector(BuildSide.Right, BuildSide.Left)
    sides.map(side => strategyCost(hasKey, side, left, right, out.rows)).minBy(c => (c.violations.size, total(c.cost)))

  /** Cost of one strategy with `build` as the materialized side. */
  def strategyCost(hash: Boolean, build: BuildSide, left: LogicalPlan, right: LogicalPlan, outRows: Double): JoinCost =
    val (built, streamed) = if build == BuildSide.Right then (right, left) else (left, right)
    val buildRows = estimator.estimate(built).rows
    val streamRows = estimator.estimate(streamed).rows
    val memory = buildRows * rowBytes(built.output)
    val limits = config.limits
    val violations = Vector(
      Option.when(buildRows > limits.maxMaterializedRows)(f"materializes ~$buildRows%.0f rows (limit ${limits.maxMaterializedRows})"),
      Option.when(memory > limits.queryMemoryBytes)(f"needs ~${memory / 1048576}%.1f MiB (limit ${limits.queryMemoryBytes / 1048576} MiB)"),
      Option.when(!hash && buildRows * streamRows > limits.maxNestedLoopComparisons)(
        f"compares ~${buildRows * streamRows}%.0f pairs (limit ${limits.maxNestedLoopComparisons})"),
      Option.when(outRows / math.max(1.0, streamRows) > limits.maxJoinFanout)(
        f"produces ~${outRows / math.max(1.0, streamRows)}%.0f matches per row (limit ${limits.maxJoinFanout})")
    ).flatten
    val cpu =
      if hash then 2 * buildRows + streamRows + 0.5 * outRows
      else 0.5 * buildRows * streamRows + 0.5 * outRows
    JoinCost(Cost.bounded(cpu, 0, memory), build, violations)

  /** Whether `part` is `left.column = right.column` across the two inputs. */
  private def isEquiKey(part: TypedExpr, leftSlots: Set[SlotId], rightSlots: Set[SlotId]): Boolean = part match
    case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Eq, TypedExpr.Column(b), _) if a.dataType == b.dataType =>
      (leftSlots(a.slot) && rightSlots(b.slot)) || (leftSlots(b.slot) && rightSlots(a.slot))
    case _ => false

  /** Width of a value of `dataType` when no statistics say otherwise. */
  private def defaultWidth(dataType: DataType): Double = dataType match
    case DataType.Bool => 1
    case DataType.Int64 | DataType.Float64 => 8
    case DataType.StringType => 24
    case DataType.Bytes => 32

  /** `log2(max(x, 2))`: never below one, so tiny inputs still cost something. */
  private def log2(x: Double): Double = math.log(math.max(x, 2.0)) / math.log(2.0)
