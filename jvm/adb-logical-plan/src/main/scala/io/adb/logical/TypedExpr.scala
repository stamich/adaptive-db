package io.adb.logical

import io.adb.model.*

/** A bound scalar expression with a statically known type. */
sealed trait TypedExpr derives CanEqual:
  /** Static type of the expression; `None` for an untyped NULL literal. */
  def dataType: Option[DataType]

  /** Every attribute the expression reads. */
  def attributes: Vector[Attribute] = this match
    case TypedExpr.Column(attribute) => Vector(attribute)
    case TypedExpr.Literal(_) => Vector.empty
    case TypedExpr.Binary(left, _, right, _) => left.attributes ++ right.attributes
    case TypedExpr.Not(expr) => expr.attributes

  /** Slots of [[attributes]], for scope checks. */
  def slots: Set[SlotId] = attributes.map(_.slot).toSet

  /** Renders the expression for EXPLAIN. */
  def display: String = this match
    case TypedExpr.Column(attribute) => attribute.display
    case TypedExpr.Literal(value) => value match
      case DbValue.NullValue => "NULL"
      case DbValue.BoolValue(v) => v.toString.toUpperCase
      case DbValue.Int64Value(v) => v.toString
      case DbValue.Float64Value(v) => v.toString
      case DbValue.StringValue(v) => s"'${v.replace("'", "''")}'"
      case DbValue.BytesValue(v) => s"<${v.length} bytes>"
    case TypedExpr.Binary(left, op, right, _) => s"(${left.display} ${op.sql} ${right.display})"
    case TypedExpr.Not(expr) => s"NOT ${expr.display}"

/** Typed expression nodes and conjunction helpers. */
object TypedExpr:
  /** Reference to an attribute of the operator's input. */
  final case class Column(attribute: Attribute) extends TypedExpr:
    /** The attribute's type. */
    def dataType = Some(attribute.dataType)
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

  /** Splits `a AND b AND c` into its conjuncts (a non-AND expression is one conjunct). */
  def conjuncts(expr: TypedExpr): Vector[TypedExpr] = expr match
    case Binary(left, BinaryOp.And, right, _) => conjuncts(left) ++ conjuncts(right)
    case other => Vector(other)

  /** Joins conjuncts with AND; `None` for an empty list. */
  def conjunction(parts: Vector[TypedExpr]): Option[TypedExpr] =
    parts.reduceOption((left, right) => Binary(left, BinaryOp.And, right, Some(DataType.Bool)))

/** Binary operator of a typed expression. */
sealed trait BinaryOp derives CanEqual:
  /** SQL spelling, used by EXPLAIN. */
  def sql: String
/** The supported binary operators. */
object BinaryOp:
  /** Equality (`=`). */
  case object Eq extends BinaryOp { val sql = "=" }
  /** Inequality (`<>` / `!=`). */
  case object Ne extends BinaryOp { val sql = "<>" }
  /** Less than (`<`). */
  case object Lt extends BinaryOp { val sql = "<" }
  /** Less than or equal (`<=`). */
  case object Le extends BinaryOp { val sql = "<=" }
  /** Greater than (`>`). */
  case object Gt extends BinaryOp { val sql = ">" }
  /** Greater than or equal (`>=`). */
  case object Ge extends BinaryOp { val sql = ">=" }
  /** Logical conjunction (`AND`). */
  case object And extends BinaryOp { val sql = "AND" }
  /** Logical disjunction (`OR`). */
  case object Or extends BinaryOp { val sql = "OR" }
