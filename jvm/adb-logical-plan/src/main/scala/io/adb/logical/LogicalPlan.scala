package io.adb.logical

import io.adb.model.*

/** Documents `LogicalPlan` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait LogicalPlan derives CanEqual
/** Documents `LogicalPlan` and its role in the Milestone 2.0.1 JVM control plane. */
object LogicalPlan:
  /** Documents `TableScan` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class TableScan(entity: Entity) extends LogicalPlan
  /** Documents `PointLookup` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class PointLookup(entity: Entity, primaryKey: Long) extends LogicalPlan
  /** Documents `Filter` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Filter(input: LogicalPlan, predicate: TypedExpr) extends LogicalPlan
  /** Documents `Project` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Project(input: LogicalPlan, fields: Vector[Field]) extends LogicalPlan
  /** Documents `Limit` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Limit(input: LogicalPlan, limit: Int) extends LogicalPlan
