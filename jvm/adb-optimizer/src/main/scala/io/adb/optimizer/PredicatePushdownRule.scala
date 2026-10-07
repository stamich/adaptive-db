package io.adb.optimizer

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*

/** Moves filter conditions as close to the scans as the join semantics allow.
  *
  * The binder puts WHERE above all joins and keeps ON conditions whole. This rule splits both
  * into conjuncts and moves each one down:
  *
  *  - a WHERE conjunct that reads only one join input is pushed into that input, except into
  *    the right side of a LEFT JOIN (that would turn null-filled rows into missing rows);
  *  - a WHERE conjunct that reads both inputs of an INNER or CROSS join becomes part of the join
  *    condition (a CROSS JOIN with a condition is an INNER JOIN), so the physical planner can
  *    use it as a hash-join key;
  *  - an ON conjunct that reads only one input of an INNER join is pushed into that input; for a
  *    LEFT JOIN only right-only conjuncts may move (filtering the right side before joining
  *    is equivalent, while a left-only ON conjunct decides null-filling and must stay).
  *
  * Pushed filters reach the scans, where [[PointLookupRule]] can turn primary-key equalities
  * into point lookups.
  */
object PredicatePushdownRule extends Rule:
  /** Rule name shown in optimizer traces. */
  val name = "PredicatePushdownRule"

  /** Rewrites the whole plan. */
  def apply(plan: LogicalPlan): LogicalPlan = rewrite(plan)

  /** Rewrites bottom-up so pushed conjuncts meet already-rewritten inputs. */
  private def rewrite(plan: LogicalPlan): LogicalPlan = plan match
    case Filter(input, predicate) => push(rewrite(input), TypedExpr.conjuncts(predicate))
    case Join(left, right, joinType, condition) => pushJoinCondition(rewrite(left), rewrite(right), joinType, condition)
    case other => mapChildren(other)(rewrite)

  /** Places `predicates` above or inside `input`. */
  private def push(input: LogicalPlan, predicates: Vector[TypedExpr]): LogicalPlan =
    if predicates.isEmpty then input
    else input match
      case Filter(inner, existing) => push(inner, TypedExpr.conjuncts(existing) ++ predicates)
      case Join(left, right, joinType, condition) =>
        val leftSlots = slots(left)
        val rightSlots = slots(right)
        val (toLeft, notLeft) = predicates.partition(_.slots.subsetOf(leftSlots))
        val (toRight, crossing) =
          if joinType == JoinType.Left then (Vector.empty, notLeft)
          else notLeft.partition(_.slots.subsetOf(rightSlots))
        val (intoJoin, above) =
          if joinType == JoinType.Left then (Vector.empty, crossing) else (crossing, Vector.empty)
        val newType = if joinType == JoinType.Cross && intoJoin.nonEmpty then JoinType.Inner else joinType
        val newCondition = TypedExpr.conjunction(condition.toVector.flatMap(TypedExpr.conjuncts) ++ intoJoin)
        filtered(Join(push(left, toLeft), push(right, toRight), newType, newCondition), above)
      case other => filtered(other, predicates)

  /** Moves single-input ON conjuncts into the inputs where that preserves the join's meaning. */
  private def pushJoinCondition(
      left: LogicalPlan,
      right: LogicalPlan,
      joinType: JoinType,
      condition: Option[TypedExpr]
  ): LogicalPlan =
    val parts = condition.toVector.flatMap(TypedExpr.conjuncts)
    val leftSlots = slots(left)
    val rightSlots = slots(right)
    val (toRight, notRight) = parts.partition(part => part.slots.nonEmpty && part.slots.subsetOf(rightSlots))
    val (toLeft, remaining) =
      if joinType == JoinType.Inner then notRight.partition(part => part.slots.nonEmpty && part.slots.subsetOf(leftSlots))
      else (Vector.empty, notRight)
    Join(push(left, toLeft), push(right, toRight), joinType, TypedExpr.conjunction(remaining))

  /** Slots an operator outputs. */
  private def slots(plan: LogicalPlan): Set[io.adb.model.SlotId] = plan.output.map(_.slot).toSet
