package io.adb.logical

import io.adb.model.*

/** Documents `BoundStatement` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait BoundStatement derives CanEqual
/** Documents `BoundCreateTable` and its role in the Milestone 2.0.1 JVM control plane. */
final case class BoundCreateTable(name: String, fields: Vector[(String, DataType, Boolean)], primaryKey: String) extends BoundStatement
/** Documents `BoundInsert` and its role in the Milestone 2.0.1 JVM control plane. */
final case class BoundInsert(entity: Entity, values: Map[FieldId, DbValue]) extends BoundStatement
/** Documents `BoundSelect` and its role in the Milestone 2.0.1 JVM control plane. */
final case class BoundSelect(entity: Entity, fields: Vector[Field], predicate: Option[TypedExpr], limit: Option[Int], asOfVersion: Option[Long]) extends BoundStatement
/** Documents `BoundUpdate` and its role in the Milestone 2.0.1 JVM control plane. */
final case class BoundUpdate(entity: Entity, assignments: Map[FieldId, DbValue], primaryKeyValue: Long) extends BoundStatement
/** Documents `BoundDelete` and its role in the Milestone 2.0.1 JVM control plane. */
final case class BoundDelete(entity: Entity, primaryKeyValue: Long) extends BoundStatement
/** Documents `BoundExplain` and its role in the Milestone 2.0.1 JVM control plane. */
final case class BoundExplain(statement: BoundStatement, analyze: Boolean) extends BoundStatement
