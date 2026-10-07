package io.adb.physical

import io.adb.logical.*
import io.adb.model.*

/** One strategy decision taken while planning, with the reason it was taken.
  *
  * Decisions are shown by EXPLAIN next to the plan, so a user (or, later, an advisor) can see
  * not only *what* the engine will do but *why*: the planning half of the adaptive loop whose
  * other half is the native runtime profile shown by EXPLAIN ANALYZE.
  *
  * @param operator the logical operator the decision is about, e.g. `INNER JOIN c with o`
  * @param choice   the chosen physical strategy, e.g. `hash_join`
  * @param reason   why it was chosen
  */
final case class PlanDecision(operator: String, choice: String, reason: String) derives CanEqual:
  /** `operator: choice (reason)`. */
  def display: String = s"$operator: $choice ($reason)"

/** Physical join strategies. */
enum JoinStrategy derives CanEqual:
  /** Hash table over the right input, streamed left input. */
  case Hash
  /** Pairwise evaluation (fallback for non-equality conditions and CROSS JOIN). */
  case NestedLoop

/** Everything the policy may look at to choose a join strategy.
  *
  * @param joinType  logical join type
  * @param equiKeys  `left = right` attribute pairs usable as hash keys
  * @param residual  remaining condition (evaluated per key match)
  * @param left      left input
  * @param right     right input
  */
final case class JoinRequest(
    joinType: JoinType,
    equiKeys: Vector[(Attribute, Attribute)],
    residual: Option[TypedExpr],
    left: LogicalPlan,
    right: LogicalPlan
)

/** Chooses physical strategies where a logical operator has several implementations.
  *
  * This is the seam where adaptivity plugs in: the default policy is rule-based, a statistics-
  * driven cost model (roadmap 2.2) or workload-driven advisors (5.x) replace it without
  * touching the planner's translation logic (open for extension, closed for modification).
  */
trait PlanningPolicy:
  /** Strategy for one join, with its reason. */
  def chooseJoin(request: JoinRequest): (JoinStrategy, String)

  /** Whether `LIMIT limit` over `ORDER BY` runs as a TopK (`true`) or as Sort + Limit, with the reason. */
  def chooseTopK(limit: Int, keys: Vector[BoundOrder]): (Boolean, String)

/** Rule-based policy of Milestone 2.1 (no statistics yet). */
object DefaultPlanningPolicy extends PlanningPolicy:
  /** Largest LIMIT run as TopK; matches the engine's TopK bound. */
  val MaxTopK: Int = 1_000_000

  /** Hash join whenever an equality key exists (O(n + m) instead of O(n × m)); the build side
    * is the right input, which LEFT JOIN requires and which, without statistics, is as good a
    * guess as any.
    */
  def chooseJoin(request: JoinRequest): (JoinStrategy, String) =
    if request.equiKeys.nonEmpty then
      val keys = request.equiKeys.map((l, r) => s"${l.name} = ${r.name}").mkString(", ")
      (JoinStrategy.Hash, s"equality key $keys; builds the right input and streams the left, O(n + m)")
    else if request.joinType == JoinType.Cross then
      (JoinStrategy.NestedLoop, "CROSS JOIN pairs every row; bounded by the comparison limit")
    else
      (JoinStrategy.NestedLoop, "no equality between the two inputs in the join condition; bounded by the comparison limit")

  /** TopK for every LIMIT the engine accepts: memory O(limit) instead of O(input). */
  def chooseTopK(limit: Int, keys: Vector[BoundOrder]): (Boolean, String) =
    if limit <= MaxTopK then (true, s"LIMIT $limit over ORDER BY keeps $limit rows in memory instead of sorting the whole input")
    else (false, s"LIMIT $limit exceeds the TopK bound $MaxTopK; full sort")
