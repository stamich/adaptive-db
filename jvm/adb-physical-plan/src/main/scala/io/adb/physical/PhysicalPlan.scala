package io.adb.physical

import io.adb.model.*

/** Physical plan understood by the native engine; mirrors the Rust `PhysicalPlan` enum. */
sealed trait PhysicalPlan derives CanEqual
/** Physical plan operators. */
object PhysicalPlan:
  /** Reads one row by its 128-bit storage key `(entity << 64) | primary key`. */
  final case class PointLookup(rowId: BigInt) extends PhysicalPlan
  /** Scans every row of every entity (diagnostics only; planners emit `EntityScan`). */
  case object Scan extends PhysicalPlan
  /** Streams the rows of one entity as a native key-range scan (Milestone 2.0.3). */
  final case class EntityScan(entityId: EntityId) extends PhysicalPlan
  /** Keeps the rows of `input` for which `predicate` is true. */
  final case class Filter(input: PhysicalPlan, predicate: PhysicalExpr) extends PhysicalPlan
  /** Restricts the rows of `input` to `fields`. */
  final case class Project(input: PhysicalPlan, fields: Vector[FieldId]) extends PhysicalPlan
  /** Stops after `limit` rows. */
  final case class Limit(input: PhysicalPlan, limit: Int) extends PhysicalPlan

/** Scalar expression evaluated by the native engine. */
sealed trait PhysicalExpr derives CanEqual
/** Expression nodes. */
object PhysicalExpr:
  /** Value of a field of the current row (NULL if absent). */
  final case class Column(fieldId: FieldId) extends PhysicalExpr
  /** Constant value. */
  final case class Literal(value: DbValue) extends PhysicalExpr
  /** Comparison or boolean combination of two expressions. */
  final case class Binary(left: PhysicalExpr, op: PhysicalBinaryOp, right: PhysicalExpr) extends PhysicalExpr
  /** Boolean negation. */
  final case class Not(expr: PhysicalExpr) extends PhysicalExpr

/** Binary operators; names map to the snake_case wire tags. */
enum PhysicalBinaryOp derives CanEqual:
  case Eq, Ne, Lt, Le, Gt, Ge, And, Or
