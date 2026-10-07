package io.adb.optimizer

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*

/** Rewrites `SELECT ... FROM t WHERE pk = <BIGINT literal>` from a filtered table scan into a primary-key point lookup. */
object PointLookupRule extends Rule:
  /** Rule name shown in optimizer traces. */
  val name = "PointLookupRule"

  /** Applies the rewrite anywhere in `plan`; plans without a matching pattern are returned unchanged. */
  def apply(plan: LogicalPlan): LogicalPlan = transform(plan)

  /** Recursively walks the plan and replaces `Filter(TableScan)` under a projection when the predicate pins the primary key. */
  private def transform(plan: LogicalPlan): LogicalPlan = plan match
    case Project(Filter(TableScan(entity), predicate), fields) =>
      pkValue(entity.primaryKey.value, predicate) match
        case Some(value) => Project(PointLookup(entity, value), fields)
        case None => Project(Filter(TableScan(entity), predicate), fields)
    case Limit(input, limit) => Limit(transform(input), limit)
    case Project(input, fields) => Project(transform(input), fields)
    case Filter(input, predicate) => Filter(transform(input), predicate)
    case other => other

  /** Extracts the key from `pk = literal` or `literal = pk`.
    *
    * @param pkId field id of the primary-key column
    * @param expr filter predicate
    * @return the pinned primary-key value, if the predicate is exactly such an equality
    */
  private def pkValue(pkId: Int, expr: TypedExpr): Option[Long] = expr match
    case TypedExpr.Binary(TypedExpr.Column(field), BinaryOp.Eq, TypedExpr.Literal(DbValue.Int64Value(v)), _) if field.id.value == pkId => Some(v)
    case TypedExpr.Binary(TypedExpr.Literal(DbValue.Int64Value(v)), BinaryOp.Eq, TypedExpr.Column(field), _) if field.id.value == pkId => Some(v)
    case _ => None
