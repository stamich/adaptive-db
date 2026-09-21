package io.adb.physical

import io.adb.model.*

/** Documents `PhysicalPlan` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait PhysicalPlan derives CanEqual
/** Documents `PhysicalPlan` and its role in the Milestone 2.0.1 JVM control plane. */
object PhysicalPlan:
  /** Documents `PointLookup` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class PointLookup(rowId: BigInt) extends PhysicalPlan
  /** Documents `Scan` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Scan extends PhysicalPlan
  /** Documents `Filter` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Filter(input: PhysicalPlan, predicate: PhysicalExpr) extends PhysicalPlan
  /** Documents `Project` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Project(input: PhysicalPlan, fields: Vector[FieldId]) extends PhysicalPlan
  /** Documents `Limit` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Limit(input: PhysicalPlan, limit: Int) extends PhysicalPlan

/** Documents `PhysicalExpr` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait PhysicalExpr derives CanEqual
/** Documents `PhysicalExpr` and its role in the Milestone 2.0.1 JVM control plane. */
object PhysicalExpr:
  /** Documents `Column` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Column(fieldId: FieldId) extends PhysicalExpr
  /** Documents `Literal` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Literal(value: DbValue) extends PhysicalExpr
  /** Documents `Binary` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Binary(left: PhysicalExpr, op: PhysicalBinaryOp, right: PhysicalExpr) extends PhysicalExpr
  /** Documents `Not` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Not(expr: PhysicalExpr) extends PhysicalExpr

/** Documents `PhysicalBinaryOp` and its role in the Milestone 2.0.1 JVM control plane. */
enum PhysicalBinaryOp derives CanEqual:
  case Eq, Ne, Lt, Le, Gt, Ge, And, Or
