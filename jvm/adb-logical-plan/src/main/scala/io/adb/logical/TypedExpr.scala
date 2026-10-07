package io.adb.logical

import io.adb.model.*

/** A bound scalar expression with a statically known type. */
sealed trait TypedExpr derives CanEqual:
  /** Static type of the expression; `None` for an untyped NULL literal. */
  def dataType: Option[DataType]

/** Typed expression nodes. */
object TypedExpr:
  /** Reference to a column of the scanned entity. */
  final case class Column(field: Field) extends TypedExpr:
    /** The column's declared type. */
    def dataType = Some(field.dataType)
  /** Constant value. */
  final case class Literal(value: DbValue) extends TypedExpr:
    /** The literal's type, or `None` for NULL. */
    def dataType = DbValue.dataType(value)
  /** Binary comparison or boolean connective.
    *
    * @param resultType type of the result (always BOOLEAN for the current operators)
    */
  final case class Binary(left: TypedExpr, op: BinaryOp, right: TypedExpr, resultType: Option[DataType]) extends TypedExpr:
    /** The precomputed result type. */
    def dataType = resultType
  /** Boolean negation. */
  final case class Not(expr: TypedExpr) extends TypedExpr:
    /** Always BOOLEAN. */
    def dataType = Some(DataType.Bool)

/** Binary operator of a typed expression. */
sealed trait BinaryOp derives CanEqual
/** The supported binary operators. */
object BinaryOp:
  /** Equality (`=`). */
  case object Eq extends BinaryOp
  /** Inequality (`<>` / `!=`). */
  case object Ne extends BinaryOp
  /** Less than (`<`). */
  case object Lt extends BinaryOp
  /** Less than or equal (`<=`). */
  case object Le extends BinaryOp
  /** Greater than (`>`). */
  case object Gt extends BinaryOp
  /** Greater than or equal (`>=`). */
  case object Ge extends BinaryOp
  /** Logical conjunction (`AND`). */
  case object And extends BinaryOp
  /** Logical disjunction (`OR`). */
  case object Or extends BinaryOp
