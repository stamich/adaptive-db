package io.adb.physical

import io.adb.logical.*
import io.adb.logical.LogicalPlan as LPlan
import io.adb.model.*
import io.adb.physical.PhysicalPlan as PPlan

/** Converts typed logical plans into the physical plan vocabulary of the native engine.
  *
  * Since Milestone 2.0.3 a table scan is a native `EntityScan`: the engine reads only the
  * entity's key range `(entity << 64, (entity + 1) << 64)` and streams it in batches, instead of
  * scanning every table and filtering on the hidden entity-id column.
  *
  * The aliases `LPlan` and `PPlan` deliberately keep logical and physical operators in separate
  * namespaces. This avoids ambiguous names such as `PointLookup`, `Filter`, `Project`, and `Limit`
  * without renaming the public ADTs themselves.
  */
object PhysicalPlanner:
  /** Builds an executable physical plan from a bound and optimized logical plan. */
  def plan(logical: LPlan): PPlan = logical match
    case LPlan.TableScan(entity) => PPlan.EntityScan(entity.id)
    case LPlan.PointLookup(entity, key) => PPlan.PointLookup(composeRowId(entity.id, key))
    case LPlan.Filter(input, predicate) => PPlan.Filter(plan(input), expr(predicate))
    case LPlan.Project(input, fields) => PPlan.Project(plan(input), fields.map(_.id))
    case LPlan.Limit(input, limit) => PPlan.Limit(plan(input), limit)

  /** Composes the 128-bit storage RowId from a 64-bit entity id and an unsigned 64-bit primary key. */
  def composeRowId(entityId: EntityId, primaryKey: Long): BigInt =
    (BigInt(entityId.value) << 64) | BigInt(java.lang.Long.toUnsignedString(primaryKey))

  /** Converts one typed logical expression into its physical execution representation. */
  private def expr(e: TypedExpr): PhysicalExpr = e match
    case TypedExpr.Column(field) => PhysicalExpr.Column(field.id)
    case TypedExpr.Literal(value) => PhysicalExpr.Literal(value)
    case TypedExpr.Not(inner) => PhysicalExpr.Not(expr(inner))
    case TypedExpr.Binary(left, op, right, _) =>
      val mapped = op match
        case BinaryOp.Eq => PhysicalBinaryOp.Eq
        case BinaryOp.Ne => PhysicalBinaryOp.Ne
        case BinaryOp.Lt => PhysicalBinaryOp.Lt
        case BinaryOp.Le => PhysicalBinaryOp.Le
        case BinaryOp.Gt => PhysicalBinaryOp.Gt
        case BinaryOp.Ge => PhysicalBinaryOp.Ge
        case BinaryOp.And => PhysicalBinaryOp.And
        case BinaryOp.Or => PhysicalBinaryOp.Or
      PhysicalExpr.Binary(expr(left), mapped, expr(right))
