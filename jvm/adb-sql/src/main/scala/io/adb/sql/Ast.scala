package io.adb.sql

/** Parsed SQL statement, before name resolution and type checking. */
sealed trait Statement derives CanEqual

/** One column definition of `CREATE TABLE`.
  *
  * @param name       column name
  * @param dataType   type name as written (resolved later by the binder)
  * @param nullable   false when `NOT NULL` (or `PRIMARY KEY`) was given
  * @param primaryKey true when the column is declared `PRIMARY KEY`
  */
final case class ColumnDef(name: String, dataType: String, nullable: Boolean, primaryKey: Boolean) derives CanEqual
/** `CREATE TABLE name (columns...)`. */
final case class CreateTable(name: String, columns: Vector[ColumnDef]) extends Statement derives CanEqual
/** `INSERT INTO table [(columns)] VALUES (values)`; without a column list the values follow declaration order. */
final case class Insert(table: String, columns: Option[Vector[String]], values: Vector[SqlExpr]) extends Statement derives CanEqual
/** `SELECT columns | * FROM table [AS OF VERSION ts] [WHERE ...] [LIMIT n]`.
  *
  * @param star        true for `SELECT *` (then `columns` is empty)
  * @param asOfVersion snapshot timestamp of an `AS OF VERSION` clause
  */
final case class Select(columns: Vector[String], star: Boolean, table: String, where: Option[SqlExpr], limit: Option[Int], asOfVersion: Option[Long]) extends Statement derives CanEqual
/** `UPDATE table SET column = value, ... WHERE ...`. */
final case class Update(table: String, assignments: Vector[(String, SqlExpr)], where: SqlExpr) extends Statement derives CanEqual
/** `DELETE FROM table WHERE ...`. */
final case class Delete(table: String, where: SqlExpr) extends Statement derives CanEqual
/** `EXPLAIN [ANALYZE] statement`. */
final case class Explain(statement: Statement, analyze: Boolean) extends Statement derives CanEqual

/** Unresolved scalar expression as written in SQL. */
sealed trait SqlExpr derives CanEqual
/** Expression nodes. */
object SqlExpr:
  /** Column reference by name. */
  final case class Column(name: String) extends SqlExpr
  /** Integer literal. */
  final case class LongLiteral(value: Long) extends SqlExpr
  /** Floating-point literal. */
  final case class DoubleLiteral(value: Double) extends SqlExpr
  /** Single-quoted string literal. */
  final case class StringLiteral(value: String) extends SqlExpr
  /** `TRUE` or `FALSE`. */
  final case class BoolLiteral(value: Boolean) extends SqlExpr
  /** `NULL`. */
  case object NullLiteral extends SqlExpr
  /** Binary comparison or boolean connective. */
  final case class Binary(left: SqlExpr, op: SqlBinaryOp, right: SqlExpr) extends SqlExpr
  /** `NOT expr`. */
  final case class Not(expr: SqlExpr) extends SqlExpr

/** Binary operator as written in SQL. */
sealed trait SqlBinaryOp derives CanEqual
/** The binary operators recognized by the parser. */
object SqlBinaryOp:
  /** `=` */
  case object Eq extends SqlBinaryOp
  /** `<>` or `!=` */
  case object Ne extends SqlBinaryOp
  /** `<` */
  case object Lt extends SqlBinaryOp
  /** `<=` */
  case object Le extends SqlBinaryOp
  /** `>` */
  case object Gt extends SqlBinaryOp
  /** `>=` */
  case object Ge extends SqlBinaryOp
  /** `AND` */
  case object And extends SqlBinaryOp
  /** `OR` */
  case object Or extends SqlBinaryOp
