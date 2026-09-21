package io.adb.logical

import io.adb.model.*

/** Documents `TypedExpr` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait TypedExpr derives CanEqual:
  /** Documents `dataType` and its role in the Milestone 2.0.1 JVM control plane. */
  def dataType: Option[DataType]

/** Documents `TypedExpr` and its role in the Milestone 2.0.1 JVM control plane. */
object TypedExpr:
  /** Documents `Column` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Column(field: Field) extends TypedExpr:
    /** Documents `dataType` and its role in the Milestone 2.0.1 JVM control plane. */
    def dataType = Some(field.dataType)
  /** Documents `Literal` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Literal(value: DbValue) extends TypedExpr:
    /** Documents `dataType` and its role in the Milestone 2.0.1 JVM control plane. */
    def dataType = DbValue.dataType(value)
  /** Documents `Binary` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Binary(left: TypedExpr, op: BinaryOp, right: TypedExpr, resultType: Option[DataType]) extends TypedExpr:
    /** Documents `dataType` and its role in the Milestone 2.0.1 JVM control plane. */
    def dataType = resultType
  /** Documents `Not` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Not(expr: TypedExpr) extends TypedExpr:
    /** Documents `dataType` and its role in the Milestone 2.0.1 JVM control plane. */
    def dataType = Some(DataType.Bool)

/** Documents `BinaryOp` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait BinaryOp derives CanEqual
/** Documents `BinaryOp` and its role in the Milestone 2.0.1 JVM control plane. */
object BinaryOp:
  /** Documents `Eq` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Eq extends BinaryOp
  /** Documents `Ne` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Ne extends BinaryOp
  /** Documents `Lt` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Lt extends BinaryOp
  /** Documents `Le` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Le extends BinaryOp
  /** Documents `Gt` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Gt extends BinaryOp
  /** Documents `Ge` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Ge extends BinaryOp
  /** Documents `And` and its role in the Milestone 2.0.1 JVM control plane. */
  case object And extends BinaryOp
  /** Documents `Or` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Or extends BinaryOp
