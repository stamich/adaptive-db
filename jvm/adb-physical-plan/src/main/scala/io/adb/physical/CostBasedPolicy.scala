package io.adb.physical

import io.adb.catalog.IndexCatalog
import io.adb.logical.*
import io.adb.optimizer.access.{AccessPath, AccessPathEnumerator}
import io.adb.optimizer.cardinality.CardinalityEstimator
import io.adb.optimizer.cost.{AccessEstimate, BuildSide, CostModel, JoinCost}

/** Statistics-driven strategy choices (the default since 2.2.3).
  *
  *  - '''joins''': every applicable strategy (hash join when an equality key exists, nested
  *    loop always) with every allowed build side (both for INNER and CROSS joins, the right
  *    input for LEFT JOIN) is costed by the [[CostModel]]; the cheapest one that stays within
  *    the engine limits wins. Building the logical left input means swapping the inputs. When
  *    no alternative fits the limits, the cheapest is chosen anyway and a warning explains
  *    which limit the estimates exceed (the engine still enforces the limit at run time).
  *  - '''ORDER BY + LIMIT''': a TopK whenever the engine accepts the limit (its memory is
  *    bounded by the limit, a full sort's by the input), explained with the estimated input size.
  *  - '''access paths''' (since 2.3.0): the sequential scan and every index scan the
  *    [[AccessPathEnumerator]] finds for the relation's filter are costed by
  *    [[CostModel.accessCost]]; the cheapest wins.
  *
  * @param estimator cardinality estimates (shared with the cost model)
  * @param costModel operator costs
  * @param indexes   indexes of the engine
  * @param snapshot  the query's `AS OF` snapshot (`None` = latest); indexes that cannot serve it
  *                  are not considered
  */
final class CostBasedPolicy(
    estimator: CardinalityEstimator,
    costModel: CostModel,
    indexes: IndexCatalog = IndexCatalog.Empty,
    snapshot: Option[Long] = None
) extends PlanningPolicy:
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
    else (true, f"keeps ${math.min(limit.toDouble, rows)}%.0f of ~$rows%.0f rows in memory instead of sorting all of them")

  /** The cheapest access path of the relation; listed by EXPLAIN when an index was considered.
    *
    * With a required order, a path that produces it (an index in that order or its reverse, or
    * the sequential scan for primary-key order) is compared, at the fraction of its rows a LIMIT
    * needs, with every other path plus the sort (TopK under a LIMIT) it would then need.
    */
  override def chooseAccess(request: AccessRequest): AccessChoice =
    val wantsOrder = request.required.nonEmpty
    val candidates = AccessPathEnumerator.candidates(
      request.relation, request.conjuncts, indexes.indexes(request.relation.entity.id), snapshot, ordered = wantsOrder, backward = wantsOrder)
    val options = candidates.map { path =>
      val full = costModel.accessCost(path)
      if wantsOrder && delivers(path, request) then
        val fraction = request.limit.fold(1.0)(k => math.min(1.0, k.toDouble / math.max(1.0, full.rows)))
        val partial = costModel.accessCost(path, fraction)
        (partial, partial.cost, true)
      else if wantsOrder then (full, full.cost + costModel.sortCost(full.rows, request.relation.columns, request.limit), false)
      else (full, full.cost, false)
    }
    val (best, bestCost, ordered) = options.minBy((_, cost, _) => costModel.total(cost))
    val others = options.filterNot(_._1 eq best)
    val sorted = if wantsOrder && !ordered then (if request.limit.isDefined then " + top_k" else " + sort") else ""
    val reason =
      f"cost ${costModel.total(bestCost)}%.1f$sorted, est. rows=${best.rows}%.0f" +
        (if ordered then "; produces the required order" else "") +
        others.minByOption((_, cost, _) => costModel.total(cost)).fold("") { (o, cost, oOrdered) =>
          val extra = if wantsOrder && !oOrdered then (if request.limit.isDefined then " + top_k" else " + sort") else ""
          f"; next best ${CostBasedPolicy.describe(o.path)}$extra cost ${costModel.total(cost)}%.1f"
        }
    AccessChoice(best.path, reason, Some(best), explain = candidates.size > 1 || ordered, ordered = ordered)

  /** Whether `path` produces the order `request` requires. */
  private def delivers(path: AccessPath, request: AccessRequest): Boolean = path match
    case scan: AccessPath.IndexScan => scan.properties.satisfies(request.required, scan.constantSlots)
    case seq: AccessPath.SeqScan => seq.properties.satisfies(request.required)

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

/** Descriptions of access paths for EXPLAIN. */
object CostBasedPolicy:
  /** `entity_scan` or `index_scan <name> [backward] [covering]`. */
  def describe(path: AccessPath): String = path match
    case AccessPath.SeqScan(_, _) => "entity_scan"
    case scan: AccessPath.IndexScan =>
      s"index_scan ${scan.index.name}" + (if scan.backward then " backward" else "") + (if scan.covering then " (covering)" else " + row_fetch")

