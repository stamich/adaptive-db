package io.adb.physical

import io.adb.logical.*
import io.adb.logical.LogicalPlan as LPlan
import io.adb.model.*
import io.adb.physical.PhysicalPlan as PPlan

/** Converts typed logical plans into the minimal physical plan vocabulary supported by Milestone 2.0.1.
  *
  * The aliases `LPlan` and `PPlan` deliberately keep logical and physical operators in separate
  * namespaces. This avoids ambiguous names such as `PointLookup`, `Filter`, `Project`, and `Limit`
  * without renaming the public ADTs themselves.
  */
object PhysicalPlanner:
  /** Builds an executable physical plan from a bound and optimized logical plan. */
  def plan(logical: LPlan): PPlan = logical match
    case LPlan.TableScan(entity) => entityScan(entity)
    case LPlan.PointLookup(entity, key) => PPlan.PointLookup(composeRowId(entity.id, key))
    case LPlan.Filter(input, predicate) => PPlan.Filter(plan(input), expr(predicate))
    case LPlan.Project(input, fields) => PPlan.Project(plan(input), fields.map(_.id))
    case LPlan.Limit(input, limit) => PPlan.Limit(plan(input), limit)

  /** Produces an entity-scoped scan by filtering the shared physical store on the hidden entity-id field. */
  private def entityScan(entity: Entity): PPlan =
    val entityPredicate = PhysicalExpr.Binary(
      PhysicalExpr.Column(SystemFields.EntityIdField),
      PhysicalBinaryOp.Eq,
      PhysicalExpr.Literal(DbValue.Int64Value(entity.id.value))
    )
    PPlan.Filter(PPlan.Scan, entityPredicate)

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
