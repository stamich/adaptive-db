package io.adb.physical

import io.adb.logical.*
import io.adb.optimizer.cardinality.CardinalityEstimator
import io.adb.optimizer.cost.{BuildSide, CostModel, JoinCost}

/** Statistics-driven strategy choices (the default since 2.2.3).
  *
  *  - '''joins''': every applicable strategy (hash join when an equality key exists, nested
  *    loop always) with every allowed build side (both for INNER and CROSS joins, the right
  *    input for LEFT JOIN) is costed by the [[CostModel]]; the cheapest one that stays within
  *    the engine limits wins. Building the logical left input means swapping the inputs. When
  *    no alternative fits the limits, the cheapest is chosen anyway and a warning explains
  *    which limit the estimates exceed (the engine still enforces the limit at run time).
  *  - '''ORDER BY + LIMIT''': a TopK whenever the engine accepts the limit, explained with the
  *    estimated input size.
  *
  * @param estimator cardinality estimates (shared with the cost model)
  * @param costModel operator costs
  */
final class CostBasedPolicy(estimator: CardinalityEstimator, costModel: CostModel) extends PlanningPolicy:
  /** Cheapest feasible strategy and build side. */
  def chooseJoin(request: JoinRequest): JoinChoice =
    val out = estimator.estimate(request.join)
    val canHash = request.equiKeys.nonEmpty && request.joinType != JoinType.Cross
    val sides = if request.joinType == JoinType.Left then Vector(BuildSide.Right) else Vector(BuildSide.Right, BuildSide.Left)
    val strategies = if canHash then Vector(JoinStrategy.Hash, JoinStrategy.NestedLoop) else Vector(JoinStrategy.NestedLoop)
    val options = for strategy <- strategies; side <- sides yield
      (strategy, side, costModel.strategyCost(strategy == JoinStrategy.Hash, side, request.left, request.right, out.rows))
    val (strategy, side, cost) = options.minBy((_, _, c) => (c.violations.size, costModel.total(c.cost)))
    val built = if side == BuildSide.Right then request.right else request.left
    val streamed = if side == BuildSide.Right then request.left else request.right
    val what = if strategy == JoinStrategy.Hash then "builds" else "materializes"
    val reason =
      f"cost ${costModel.total(cost.cost)}%.1f: $what ${aliases(built)} (~${estimator.estimate(built).rows}%.0f rows, " +
        f"~${cost.cost.memoryBytes / 1024}%.1f KiB) and streams ${aliases(streamed)} (~${estimator.estimate(streamed).rows}%.0f rows); " +
        f"est. ${out.rows}%.0f rows, confidence ${out.confidence}%.2f from ${out.source.label}" +
        (if side == BuildSide.Left then "; inputs swapped so the smaller side is built" else "") +
        alternative(options.filterNot(_._3 eq cost).map(_._3))
    val warnings =
      if cost.violations.isEmpty then Vector.empty
      else Vector(s"${request.joinType.toString.toUpperCase} JOIN ${aliases(request.left)} with ${aliases(request.right)}: " +
        s"no strategy fits the engine limits (${cost.violations.mkString("; ")}); the cheapest was chosen and may be aborted")
    JoinChoice(strategy, side == BuildSide.Left, reason, warnings)

  /** TopK unless the limit exceeds the engine's TopK bound. */
  def chooseTopK(limit: Int, keys: Vector[BoundOrder], input: LogicalPlan): (Boolean, String) =
    val rows = estimator.estimate(input).rows
    if limit > DefaultPlanningPolicy.MaxTopK then (false, s"LIMIT $limit exceeds the TopK bound ${DefaultPlanningPolicy.MaxTopK}; full sort")
    else
      val sort = costModel.total(costModel.ownCost(LogicalPlan.Sort(input, keys)))
      val topK = costModel.total(costModel.topKCost(input, limit))
      (true, f"keeps ${math.min(limit.toDouble, rows)}%.0f of ~$rows%.0f rows; cost ${topK}%.1f vs full sort ${sort}%.1f")

  /** `; next best: cost N` for the cheapest rejected alternative, if any. */
  private def alternative(others: Vector[JoinCost]): String =
    others.minByOption(c => (c.violations.size, costModel.total(c.cost))).fold("") { c =>
      f"; next best cost ${costModel.total(c.cost)}%.1f" + (if c.violations.nonEmpty then " (exceeds limits)" else "")
    }

  /** Aliases of the relations an input reads. */
  private def aliases(plan: LogicalPlan): String = plan match
    case LogicalPlan.TableScan(relation) => relation.alias
    case LogicalPlan.PointLookup(relation, _) => relation.alias
    case other => other.children.map(aliases).mkString(",")
