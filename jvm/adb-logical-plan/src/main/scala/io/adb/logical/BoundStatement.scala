package io.adb.logical

import io.adb.model.*

/** A statement whose names are resolved against the catalog and whose expressions are type-checked. */
sealed trait BoundStatement derives CanEqual
/** `CREATE TABLE`: entity name, `(column, type, nullable)` definitions, and primary-key column name. */
final case class BoundCreateTable(name: String, fields: Vector[(String, DataType, Boolean)], primaryKey: String) extends BoundStatement
/** `INSERT` of one row: values keyed by field id. */
final case class BoundInsert(entity: Entity, values: Map[FieldId, DbValue]) extends BoundStatement
/** `UPDATE` of one row identified by its primary key: new values keyed by field id. */
final case class BoundUpdate(entity: Entity, assignments: Map[FieldId, DbValue], primaryKeyValue: Long) extends BoundStatement
/** `DELETE` of one row identified by its primary key. */
final case class BoundDelete(entity: Entity, primaryKeyValue: Long) extends BoundStatement
/** `EXPLAIN [ANALYZE]` of an inner statement. */
final case class BoundExplain(statement: BoundStatement, analyze: Boolean) extends BoundStatement

/** One relation instance of a SELECT (a FROM or JOIN item).
  *
  * @param id      query-scoped relation id (position in the FROM clause)
  * @param entity  the entity read
  * @param alias   name the query uses for it (alias, or the table name)
  * @param columns attributes the query reads from it, in slot order; only referenced fields
  *                are listed, so the native scan reads nothing else
  */
final case class BoundRelation(id: RelationId, entity: Entity, alias: String, columns: Vector[Attribute]) derives CanEqual

/** Join semantics of the logical plan. */
enum JoinType derives CanEqual:
  /** Matching pairs only. */
  case Inner
  /** Matching pairs plus unmatched left rows with NULL right attributes. */
  case Left
  /** Every pair (no condition). */
  case Cross

/** One join of a SELECT, applied left-deep in FROM order.
  *
  * @param joinType  INNER, LEFT or CROSS
  * @param relation  the joined relation
  * @param condition the ON condition (absent for CROSS)
  */
final case class BoundJoin(joinType: JoinType, relation: BoundRelation, condition: Option[TypedExpr]) derives CanEqual

/** One aggregate computed by a SELECT.
  *
  * @param function the aggregate function
  * @param input    the aggregated attribute; `None` for `COUNT(*)`
  * @param output   synthetic attribute holding the result
  */
final case class BoundAggregate(function: AggregateFunction, input: Option[Attribute], output: Attribute) derives CanEqual:
  /** `SUM(o.amount#3)` style rendering for EXPLAIN. */
  def display: String = s"${function.sqlName}(${input.fold("*")(_.display)})"

/** One ORDER BY key. */
final case class BoundOrder(attribute: Attribute, descending: Boolean) derives CanEqual:
  /** `total#5 DESC` style rendering for EXPLAIN. */
  def display: String = s"${attribute.display}${if descending then " DESC" else ""}"

/** One output column of a SELECT: display name and the attribute it shows. */
final case class OutputColumn(name: String, attribute: Attribute) derives CanEqual

/** `SELECT`, fully resolved.
  *
  * @param base        first FROM relation
  * @param joins       joins in FROM order
  * @param predicate   WHERE predicate
  * @param groupBy     grouping attributes
  * @param aggregates  aggregates to compute (including ones only used by ORDER BY)
  * @param output      projected columns, in output order
  * @param orderBy     ORDER BY keys
  * @param limit       LIMIT row count
  * @param asOfVersion snapshot timestamp of an `AS OF VERSION` clause
  */
final case class BoundSelect(
    base: BoundRelation,
    joins: Vector[BoundJoin],
    predicate: Option[TypedExpr],
    groupBy: Vector[Attribute],
    aggregates: Vector[BoundAggregate],
    output: Vector[OutputColumn],
    orderBy: Vector[BoundOrder],
    limit: Option[Int],
    asOfVersion: Option[Long]
) extends BoundStatement:
  /** Every relation instance, in FROM order. */
  def relations: Vector[BoundRelation] = base +: joins.map(_.relation)
  /** Whether the query aggregates (GROUP BY or aggregate functions). */
  def isAggregate: Boolean = groupBy.nonEmpty || aggregates.nonEmpty
