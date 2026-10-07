package io.adb.logical

import io.adb.model.*

/** Relational operator tree produced from a bound SELECT; input to the optimizer. */
sealed trait LogicalPlan derives CanEqual
/** Logical operators. */
object LogicalPlan:
  /** Reads every row of one entity. */
  final case class TableScan(entity: Entity) extends LogicalPlan
  /** Reads one row of an entity by primary key (produced by [[io.adb.optimizer.PointLookupRule]]). */
  final case class PointLookup(entity: Entity, primaryKey: Long) extends LogicalPlan
  /** Keeps the rows of `input` for which `predicate` is true. */
  final case class Filter(input: LogicalPlan, predicate: TypedExpr) extends LogicalPlan
  /** Restricts the rows of `input` to `fields`. */
  final case class Project(input: LogicalPlan, fields: Vector[Field]) extends LogicalPlan
  /** Stops after `limit` rows. */
  final case class Limit(input: LogicalPlan, limit: Int) extends LogicalPlan
