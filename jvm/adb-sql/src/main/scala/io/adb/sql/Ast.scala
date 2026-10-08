package io.adb.sql

/** Parsed SQL statement, before name resolution and type checking. */
sealed trait Statement derives CanEqual

/** One column definition of `CREATE TABLE`.
  *
  * @param name       column name
  * @param dataType   type name as written (resolved later by the binder)
  * @param nullable   false when `NOT NULL` (or `PRIMARY KEY`) was given
  * @param primaryKey true when the column is declared `PRIMARY KEY`
  * @param references `REFERENCES table(column) NOT ENFORCED` target, if given
  */
final case class ColumnDef(
    name: String,
    dataType: String,
    nullable: Boolean,
    primaryKey: Boolean,
    references: Option[ColumnReference] = None
) derives CanEqual
/** Target of a foreign-key hint: `table(column)` as written. */
final case class ColumnReference(table: String, column: String) derives CanEqual
/** `CREATE TABLE name (columns...)`. */
final case class CreateTable(name: String, columns: Vector[ColumnDef]) extends Statement derives CanEqual
/** `INSERT INTO table [(columns)] VALUES (values)`; without a column list the values follow declaration order. */
final case class Insert(table: String, columns: Option[Vector[String]], values: Vector[SqlExpr]) extends Statement derives CanEqual

/** `SELECT ... FROM ... [JOIN ...] [AS OF VERSION ts] [WHERE ...] [GROUP BY ...] [ORDER BY ...] [LIMIT n]`.
  *
  * @param items       select list; empty when `star` is set
  * @param star        true for `SELECT *`
  * @param from        FROM clause with its joins
  * @param where       WHERE predicate
  * @param groupBy     GROUP BY column references
  * @param orderBy     ORDER BY items
  * @param limit       LIMIT row count
  * @param asOfVersion snapshot timestamp of an `AS OF VERSION` clause
  */
final case class Select(
    items: Vector[SelectItem],
    star: Boolean,
    from: FromClause,
    where: Option[SqlExpr],
    groupBy: Vector[SqlExpr.Column],
    orderBy: Vector[OrderItem],
    limit: Option[Int],
    asOfVersion: Option[Long]
) extends Statement derives CanEqual

/** `UPDATE table SET column = value, ... WHERE ...`. */
final case class Update(table: String, assignments: Vector[(String, SqlExpr)], where: SqlExpr) extends Statement derives CanEqual
/** `DELETE FROM table WHERE ...`. */
final case class Delete(table: String, where: SqlExpr) extends Statement derives CanEqual
/** `ANALYZE [table]`: collect optimizer statistics of one table, or of every table. */
final case class Analyze(table: Option[String]) extends Statement derives CanEqual
/** `SET name = value` (or `SET name TO value`): a session setting such as `optimizer`. */
final case class SetOption(name: String, value: String) extends Statement derives CanEqual
/** `EXPLAIN [ANALYZE] statement`. */
final case class Explain(statement: Statement, analyze: Boolean) extends Statement derives CanEqual

/** One entry of a select list: an expression with an optional `AS alias`. */
final case class SelectItem(expr: SqlExpr, alias: Option[String]) derives CanEqual
/** One ORDER BY entry. */
final case class OrderItem(expr: SqlExpr, descending: Boolean) derives CanEqual
/** A table in FROM or JOIN with its optional alias. */
final case class TableRef(table: String, alias: Option[String]) derives CanEqual
/** FROM clause: the first table followed by left-deep joins. */
final case class FromClause(base: TableRef, joins: Vector[JoinClause]) derives CanEqual
/** One `[INNER|LEFT [OUTER]|CROSS] JOIN table [alias] [ON condition]`; CROSS joins have no condition. */
final case class JoinClause(kind: JoinKind, table: TableRef, on: Option[SqlExpr]) derives CanEqual

/** Join kinds of the SQL grammar. */
enum JoinKind derives CanEqual:
  /** `[INNER] JOIN ... ON`. */
  case Inner
  /** `LEFT [OUTER] JOIN ... ON`. */
  case Left
  /** `CROSS JOIN` (every pair). */
  case Cross

/** Aggregate function names of the SQL grammar. */
enum SqlAggregate derives CanEqual:
  /** `COUNT`. */
  case Count
  /** `SUM`. */
  case Sum
  /** `MIN`. */
  case Min
  /** `MAX`. */
  case Max
  /** `AVG`. */
  case Avg

/** Companion with name lookup. */
object SqlAggregate:
  /** The aggregate named `name` (case-insensitive), if any. */
  def named(name: String): Option[SqlAggregate] = values.find(_.toString.equalsIgnoreCase(name))

/** Unresolved scalar expression as written in SQL. */
sealed trait SqlExpr derives CanEqual
/** Expression nodes. */
object SqlExpr:
  /** Column reference, optionally qualified: `name` or `qualifier.name`. */
  final case class Column(name: String, qualifier: Option[String] = None) extends SqlExpr:
    /** The reference as written. */
    def display: String = qualifier.fold(name)(q => s"$q.$name")
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
  /** Aggregate call: `COUNT(*)` has no argument, every other call has one. */
  final case class AggregateCall(function: SqlAggregate, argument: Option[SqlExpr]) extends SqlExpr:
    /** The call as written, e.g. `SUM(o.amount)`. */
    def display: String = s"${function.toString.toUpperCase}(${argument.fold("*")(SqlExpr.render)})"

  /** Renders an expression back to SQL-like text (used in messages and display names). */
  def render(expr: SqlExpr): String = expr match
    case column: Column => column.display
    case LongLiteral(v) => v.toString
    case DoubleLiteral(v) => v.toString
    case StringLiteral(v) => s"'${v.replace("'", "''")}'"
    case BoolLiteral(v) => v.toString.toUpperCase
    case NullLiteral => "NULL"
    case Binary(l, op, r) => s"(${render(l)} ${op.sql} ${render(r)})"
    case Not(e) => s"NOT ${render(e)}"
    case call: AggregateCall => call.display

/** Binary operator as written in SQL. */
sealed trait SqlBinaryOp derives CanEqual:
  /** SQL spelling of the operator. */
  def sql: String
/** The binary operators recognized by the parser. */
object SqlBinaryOp:
  /** `=` */
  case object Eq extends SqlBinaryOp { val sql = "=" }
  /** `<>` or `!=` */
  case object Ne extends SqlBinaryOp { val sql = "<>" }
  /** `<` */
  case object Lt extends SqlBinaryOp { val sql = "<" }
  /** `<=` */
  case object Le extends SqlBinaryOp { val sql = "<=" }
  /** `>` */
  case object Gt extends SqlBinaryOp { val sql = ">" }
  /** `>=` */
  case object Ge extends SqlBinaryOp { val sql = ">=" }
  /** `AND` */
  case object And extends SqlBinaryOp { val sql = "AND" }
  /** `OR` */
  case object Or extends SqlBinaryOp { val sql = "OR" }
