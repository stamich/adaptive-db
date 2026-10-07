package io.adb.optimizer

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*

/** Rewrites a filtered table scan whose predicate pins the primary key
  * (`pk = <BIGINT literal>`, possibly among other AND-ed conditions) into a point lookup, keeping
  * the other conditions as a filter above it. Works on every scan of the plan, including the
  * inputs of joins once [[PredicatePushdownRule]] has pushed the conditions down.
  */
object PointLookupRule extends Rule:
  /** Rule name shown in optimizer traces. */
  val name = "PointLookupRule"

  /** Applies the rewrite anywhere in `plan`; plans without a matching pattern are returned unchanged. */
  def apply(plan: LogicalPlan): LogicalPlan = transform(plan)

  /** Recursively replaces `Filter(TableScan)` when one conjunct pins the primary key. */
  private def transform(plan: LogicalPlan): LogicalPlan = plan match
    case Filter(TableScan(relation), predicate) =>
      val parts = TypedExpr.conjuncts(predicate)
      parts.iterator.zipWithIndex.collectFirst(Function.unlift { case (part, index) =>
        pkValue(relation, part).map(_ -> index)
      }) match
        case Some((key, index)) => filtered(PointLookup(relation, key), parts.patch(index, Nil, 1))
        case None => plan
    case other => mapChildren(other)(transform)

  /** The key of `pk = literal` or `literal = pk` on `relation`'s primary key, if `expr` is one. */
  private def pkValue(relation: BoundRelation, expr: TypedExpr): Option[Long] = expr match
    case TypedExpr.Binary(TypedExpr.Column(attribute), BinaryOp.Eq, TypedExpr.Literal(DbValue.Int64Value(v)), _)
        if isPrimaryKey(relation, attribute) => Some(v)
    case TypedExpr.Binary(TypedExpr.Literal(DbValue.Int64Value(v)), BinaryOp.Eq, TypedExpr.Column(attribute), _)
        if isPrimaryKey(relation, attribute) => Some(v)
    case _ => None

  /** Whether `attribute` is the primary-key column of this relation instance. */
  private def isPrimaryKey(relation: BoundRelation, attribute: Attribute): Boolean =
    attribute.origin == ColumnOrigin.Stored(relation.id, relation.entity.id, relation.entity.primaryKey)
