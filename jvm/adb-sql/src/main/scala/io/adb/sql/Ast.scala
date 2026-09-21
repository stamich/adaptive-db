package io.adb.sql

/** Documents `Statement` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait Statement derives CanEqual

/** Documents `ColumnDef` and its role in the Milestone 2.0.1 JVM control plane. */
final case class ColumnDef(name: String, dataType: String, nullable: Boolean, primaryKey: Boolean) derives CanEqual
/** Documents `CreateTable` and its role in the Milestone 2.0.1 JVM control plane. */
final case class CreateTable(name: String, columns: Vector[ColumnDef]) extends Statement derives CanEqual
/** Documents `Insert` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Insert(table: String, columns: Option[Vector[String]], values: Vector[SqlExpr]) extends Statement derives CanEqual
/** Documents `Select` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Select(columns: Vector[String], star: Boolean, table: String, where: Option[SqlExpr], limit: Option[Int], asOfVersion: Option[Long]) extends Statement derives CanEqual
/** Documents `Update` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Update(table: String, assignments: Vector[(String, SqlExpr)], where: SqlExpr) extends Statement derives CanEqual
/** Documents `Delete` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Delete(table: String, where: SqlExpr) extends Statement derives CanEqual
/** Documents `Explain` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Explain(statement: Statement, analyze: Boolean) extends Statement derives CanEqual

/** Documents `SqlExpr` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait SqlExpr derives CanEqual
/** Documents `SqlExpr` and its role in the Milestone 2.0.1 JVM control plane. */
object SqlExpr:
  /** Documents `Column` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Column(name: String) extends SqlExpr
  /** Documents `LongLiteral` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class LongLiteral(value: Long) extends SqlExpr
  /** Documents `DoubleLiteral` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class DoubleLiteral(value: Double) extends SqlExpr
  /** Documents `StringLiteral` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class StringLiteral(value: String) extends SqlExpr
  /** Documents `BoolLiteral` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class BoolLiteral(value: Boolean) extends SqlExpr
  /** Documents `NullLiteral` and its role in the Milestone 2.0.1 JVM control plane. */
  case object NullLiteral extends SqlExpr
  /** Documents `Binary` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Binary(left: SqlExpr, op: SqlBinaryOp, right: SqlExpr) extends SqlExpr
  /** Documents `Not` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Not(expr: SqlExpr) extends SqlExpr

/** Documents `SqlBinaryOp` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait SqlBinaryOp derives CanEqual
/** Documents `SqlBinaryOp` and its role in the Milestone 2.0.1 JVM control plane. */
object SqlBinaryOp:
  /** Documents `Eq` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Eq extends SqlBinaryOp
  /** Documents `Ne` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Ne extends SqlBinaryOp
  /** Documents `Lt` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Lt extends SqlBinaryOp
  /** Documents `Le` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Le extends SqlBinaryOp
  /** Documents `Gt` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Gt extends SqlBinaryOp
  /** Documents `Ge` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Ge extends SqlBinaryOp
  /** Documents `And` and its role in the Milestone 2.0.1 JVM control plane. */
  case object And extends SqlBinaryOp
  /** Documents `Or` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Or extends SqlBinaryOp
