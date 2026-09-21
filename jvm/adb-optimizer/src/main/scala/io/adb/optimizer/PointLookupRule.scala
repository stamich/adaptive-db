package io.adb.optimizer

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*

/** Documents `PointLookupRule` and its role in the Milestone 2.0.1 JVM control plane. */
object PointLookupRule extends Rule:
  val name = "PointLookupRule"

  /** Documents `apply` and its role in the Milestone 2.0.1 JVM control plane. */
  def apply(plan: LogicalPlan): LogicalPlan = transform(plan)

  /** Documents `transform` and its role in the Milestone 2.0.1 JVM control plane. */
  private def transform(plan: LogicalPlan): LogicalPlan = plan match
    case Project(Filter(TableScan(entity), predicate), fields) =>
      pkValue(entity.primaryKey.value, predicate) match
        case Some(value) => Project(PointLookup(entity, value), fields)
        case None => Project(Filter(TableScan(entity), predicate), fields)
    case Limit(input, limit) => Limit(transform(input), limit)
    case Project(input, fields) => Project(transform(input), fields)
    case Filter(input, predicate) => Filter(transform(input), predicate)
    case other => other

  /** Documents `pkValue` and its role in the Milestone 2.0.1 JVM control plane. */
  private def pkValue(pkId: Int, expr: TypedExpr): Option[Long] = expr match
    case TypedExpr.Binary(TypedExpr.Column(field), BinaryOp.Eq, TypedExpr.Literal(DbValue.Int64Value(v)), _) if field.id.value == pkId => Some(v)
    case TypedExpr.Binary(TypedExpr.Literal(DbValue.Int64Value(v)), BinaryOp.Eq, TypedExpr.Column(field), _) if field.id.value == pkId => Some(v)
    case _ => None
