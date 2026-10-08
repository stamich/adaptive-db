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
  * @param join     the logical join (inputs, type and condition)
  * @param equiKeys `left = right` attribute pairs usable as hash keys
  * @param residual remaining condition (evaluated per key match)
  */
final case class JoinRequest(join: LogicalPlan.Join, equiKeys: Vector[(Attribute, Attribute)], residual: Option[TypedExpr]):
  /** Logical join type. */
  def joinType: JoinType = join.joinType
  /** Left input. */
  def left: LogicalPlan = join.left
  /** Right input. */
  def right: LogicalPlan = join.right

/** A join strategy decision.
  *
  * @param strategy   hash or nested-loop join
  * @param swapInputs run the join with its inputs exchanged, so the engine materializes the
  *                   logical left input (only for INNER joins; the engine always builds or
  *                   materializes its right input)
  * @param reason     why, for EXPLAIN
  * @param warnings   problems the planner could not avoid (e.g. every strategy exceeds an
  *                   engine limit), shown by EXPLAIN
  */
final case class JoinChoice(strategy: JoinStrategy, swapInputs: Boolean, reason: String, warnings: Vector[String] = Vector.empty) derives CanEqual

/** Chooses physical strategies where a logical operator has several implementations.
  *
  * This is the seam where adaptivity plugs in: [[DefaultPlanningPolicy]] is rule-based,
  * [[CostBasedPolicy]] uses statistics and the cost model, workload-driven advisors (5.x) can
  * replace both without touching the planner's translation logic.
  */
trait PlanningPolicy:
  /** Strategy for one join, with its reason. */
  def chooseJoin(request: JoinRequest): JoinChoice

  /** Whether `LIMIT limit` over `ORDER BY` runs as a TopK (`true`) or as Sort + Limit, with the
    * reason; `input` is the plan being ordered.
    */
  def chooseTopK(limit: Int, keys: Vector[BoundOrder], input: LogicalPlan): (Boolean, String)

/** Rule-based policy of Milestone 2.1 (no statistics), used by `SET optimizer = rule`. */
object DefaultPlanningPolicy extends PlanningPolicy:
  /** Largest LIMIT run as TopK; matches the engine's TopK bound. */
  val MaxTopK: Int = 1_000_000

  /** Hash join whenever an equality key exists (O(n + m) instead of O(n × m)); the build side
    * is the right input, which LEFT JOIN requires and which, without statistics, is as good a
    * guess as any.
    */
  def chooseJoin(request: JoinRequest): JoinChoice =
    if request.equiKeys.nonEmpty then
      val keys = request.equiKeys.map((l, r) => s"${l.name} = ${r.name}").mkString(", ")
      JoinChoice(JoinStrategy.Hash, false, s"equality key $keys; builds the right input and streams the left, O(n + m)")
    else if request.joinType == JoinType.Cross then
      JoinChoice(JoinStrategy.NestedLoop, false, "CROSS JOIN pairs every row; bounded by the comparison limit")
    else
      JoinChoice(JoinStrategy.NestedLoop, false, "no equality between the two inputs in the join condition; bounded by the comparison limit")

  /** TopK for every LIMIT the engine accepts: memory O(limit) instead of O(input). */
  def chooseTopK(limit: Int, keys: Vector[BoundOrder], input: LogicalPlan): (Boolean, String) =
    if limit <= MaxTopK then (true, s"LIMIT $limit over ORDER BY keeps $limit rows in memory instead of sorting the whole input")
    else (false, s"LIMIT $limit exceeds the TopK bound $MaxTopK; full sort")
