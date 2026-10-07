package io.adb.physical

import io.adb.model.*

/** Physical plan understood by the native engine; mirrors the Rust `PhysicalPlan` enum
  * (plan wire v2, see docs/plan-wire-format.md). Every node works on [[SlotId]]s; only the
  * leaves mention storage field ids, in their [[ScanColumn]] mappings.
  */
sealed trait PhysicalPlan derives CanEqual:
  /** Input plans, left to right. */
  def children: Vector[PhysicalPlan] = this match
    case PhysicalPlan.Filter(input, _) => Vector(input)
    case PhysicalPlan.Project(input, _) => Vector(input)
    case PhysicalPlan.Limit(input, _) => Vector(input)
    case PhysicalPlan.HashJoin(left, right, _, _, _) => Vector(left, right)
    case PhysicalPlan.NestedLoopJoin(left, right, _, _) => Vector(left, right)
    case PhysicalPlan.Aggregate(input, _, _) => Vector(input)
    case PhysicalPlan.Sort(input, _) => Vector(input)
    case PhysicalPlan.TopK(input, _, _) => Vector(input)
    case _ => Vector.empty

/** Physical plan operators. */
object PhysicalPlan:
  /** Reads one row by its 128-bit storage key `(entity << 64) | primary key`. */
  final case class PointLookup(rowId: BigInt, columns: Vector[ScanColumn]) extends PhysicalPlan
  /** Scans every row of every entity (diagnostics only; planners emit `EntityScan`). */
  final case class Scan(columns: Vector[ScanColumn]) extends PhysicalPlan
  /** Streams the rows of one entity as a native key-range scan. */
  final case class EntityScan(entityId: EntityId, columns: Vector[ScanColumn]) extends PhysicalPlan
  /** Keeps the rows of `input` for which `predicate` is true. */
  final case class Filter(input: PhysicalPlan, predicate: PhysicalExpr) extends PhysicalPlan
  /** Selects and orders the output slots. */
  final case class Project(input: PhysicalPlan, slots: Vector[SlotId]) extends PhysicalPlan
  /** Stops after `limit` rows. */
  final case class Limit(input: PhysicalPlan, limit: Int) extends PhysicalPlan
  /** Hash equi-join: builds on `right`, streams `left`. */
  final case class HashJoin(
      left: PhysicalPlan,
      right: PhysicalPlan,
      joinType: PhysicalJoinType,
      keys: Vector[JoinKey],
      residual: Option[PhysicalExpr]
  ) extends PhysicalPlan
  /** Nested-loop join for conditions without equality keys (and CROSS JOIN). */
  final case class NestedLoopJoin(
      left: PhysicalPlan,
      right: PhysicalPlan,
      joinType: PhysicalJoinType,
      predicate: Option[PhysicalExpr]
  ) extends PhysicalPlan
  /** Hash aggregation: grouping slots plus aggregates. */
  final case class Aggregate(input: PhysicalPlan, groupBy: Vector[SlotId], aggregates: Vector[AggregateSpec]) extends PhysicalPlan
  /** Full sort. */
  final case class Sort(input: PhysicalPlan, keys: Vector[SortKey]) extends PhysicalPlan
  /** The first `limit` rows in `keys` order, without sorting everything. */
  final case class TopK(input: PhysicalPlan, keys: Vector[SortKey], limit: Int) extends PhysicalPlan

/** Leaf mapping of one stored field to the slot it is read into. */
final case class ScanColumn(fieldId: FieldId, slot: SlotId) derives CanEqual
/** Physical join semantics (a CROSS JOIN is an INNER nested-loop join without predicate). */
enum PhysicalJoinType derives CanEqual:
  /** Matching pairs only. */
  case Inner
  /** Matching pairs plus unmatched left rows with NULL right slots. */
  case Left
/** One equality `left = right` of a hash join; `left` is produced by the left input. */
final case class JoinKey(left: SlotId, right: SlotId) derives CanEqual
/** One aggregate: `function(input) -> output`; `input` is empty only for `COUNT(*)`. */
final case class AggregateSpec(function: AggregateFunction, input: Option[SlotId], output: SlotId) derives CanEqual
/** One sort key (NULLs last ascending, first descending). */
final case class SortKey(slot: SlotId, descending: Boolean) derives CanEqual

/** Scalar expression evaluated by the native engine. */
sealed trait PhysicalExpr derives CanEqual
/** Expression nodes. */
object PhysicalExpr:
  /** Value of a slot of the current row. */
  final case class Slot(slot: SlotId) extends PhysicalExpr
  /** Constant value. */
  final case class Literal(value: DbValue) extends PhysicalExpr
  /** Comparison or boolean combination of two expressions. */
  final case class Binary(left: PhysicalExpr, op: PhysicalBinaryOp, right: PhysicalExpr) extends PhysicalExpr
  /** Boolean negation. */
  final case class Not(expr: PhysicalExpr) extends PhysicalExpr

/** Binary operators; names map to the snake_case wire tags. */
enum PhysicalBinaryOp derives CanEqual:
  /** Comparisons (`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`) and boolean connectives (`And`, `Or`). */
  case Eq, Ne, Lt, Le, Gt, Ge, And, Or
