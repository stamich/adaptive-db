package io.adb.logical

import io.adb.model.*

/** A statement whose names are resolved against the catalog and whose expressions are type-checked. */
sealed trait BoundStatement derives CanEqual
/** `CREATE TABLE`: entity name, `(column, type, nullable)` definitions, and primary-key column name. */
final case class BoundCreateTable(name: String, fields: Vector[(String, DataType, Boolean)], primaryKey: String) extends BoundStatement
/** `INSERT` of one row: values keyed by field id. */
final case class BoundInsert(entity: Entity, values: Map[FieldId, DbValue]) extends BoundStatement
/** `SELECT`: projected fields, optional predicate and limit, and optional `AS OF` snapshot timestamp. */
final case class BoundSelect(entity: Entity, fields: Vector[Field], predicate: Option[TypedExpr], limit: Option[Int], asOfVersion: Option[Long]) extends BoundStatement
/** `UPDATE` of one row identified by its primary key: new values keyed by field id. */
final case class BoundUpdate(entity: Entity, assignments: Map[FieldId, DbValue], primaryKeyValue: Long) extends BoundStatement
/** `DELETE` of one row identified by its primary key. */
final case class BoundDelete(entity: Entity, primaryKeyValue: Long) extends BoundStatement
/** `EXPLAIN [ANALYZE]` of an inner statement. */
final case class BoundExplain(statement: BoundStatement, analyze: Boolean) extends BoundStatement
