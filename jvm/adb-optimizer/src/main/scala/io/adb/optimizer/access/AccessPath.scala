package io.adb.optimizer.access

import io.adb.catalog.IndexInfo
import io.adb.logical.*
import io.adb.model.*

/** Order of the rows a physical operator produces: sort keys, most significant first. */
final case class PhysicalProperties(ordering: Vector[SortOrder]) derives CanEqual:
  /** Whether rows in this order are also in `required` order, ignoring required keys that are
    * constant in the output (an equality-bound column cannot change the order).
    */
  def satisfies(required: Vector[SortOrder], constants: Set[SlotId] = Set.empty): Boolean =
    val effective = required.filterNot(key => constants.contains(key.slot))
    effective.size <= ordering.size && effective.zip(ordering).forall(_ == _)

/** Construction helpers of [[PhysicalProperties]]. */
object PhysicalProperties:
  /** No known order. */
  val Unordered: PhysicalProperties = PhysicalProperties(Vector.empty)

/** One sort key of an ordering (NULLs last ascending, first descending, as the engine sorts). */
final case class SortOrder(slot: SlotId, descending: Boolean) derives CanEqual

/** How the rows of one relation are read.
  *
  * Every access path produces the rows of its relation that satisfy the conjuncts it applies;
  * the planner keeps the remaining conjuncts as a filter above it.
  */
sealed trait AccessPath derives CanEqual:
  /** The relation read. */
  def relation: BoundRelation
  /** Conjuncts applied above the path (the ones it does not apply itself). */
  def residual: Vector[TypedExpr]
  /** Order of the rows the path produces. */
  def properties: PhysicalProperties

/** The access paths. */
object AccessPath:
  /** Reads every row of the entity in primary-key order (storage keys are order-preserving
    * since 2.3.0, so this is primary-key value order).
    */
  final case class SeqScan(relation: BoundRelation, residual: Vector[TypedExpr]) extends AccessPath:
    /** Primary-key order, when the primary key is read. */
    def properties: PhysicalProperties = primaryKeyOrder(relation, descending = false)

  /** Reads the entries of an index (forward or backward), then — unless every column the query
    * reads is a key or covered column — the remaining columns of each row (a row fetch).
    *
    * @param matched   what the index applies
    * @param backward  whether the scan runs in reverse index order
    * @param covering  whether the index holds every column the query reads
    */
  final case class IndexScan(relation: BoundRelation, matched: IndexMatch, backward: Boolean, covering: Boolean) extends AccessPath:
    /** The conjuncts the index does not apply. */
    def residual: Vector[TypedExpr] = matched.residual
    /** The index. */
    def index: IndexInfo = matched.index
    /** Index order of the key columns after the equality prefix, then primary-key order (index
      * entries of equal keys are ordered by row id), all reversed for a backward scan.
      */
    def properties: PhysicalProperties =
      val keys = matched.index.columns.drop(matched.equalities.size).map { column =>
        attributeOf(relation, column.field).map(a => SortOrder(a.slot, column.descending != backward))
      }
      val known = keys.takeWhile(_.isDefined).flatten
      val tail = if known.size == keys.size then primaryKeyOrder(relation, descending = backward).ordering else Vector.empty
      PhysicalProperties(known ++ tail)

    /** Slots whose value is the same for every produced row (the equality-bound key columns). */
    def constantSlots: Set[SlotId] =
      matched.equalities.flatMap((column, _, _) => attributeOf(relation, column.field).map(_.slot)).toSet

  /** The attribute through which `relation` reads `field`, if the query reads it. */
  def attributeOf(relation: BoundRelation, field: FieldId): Option[Attribute] =
    relation.columns.find(_.origin match
      case ColumnOrigin.Stored(_, _, f) => f == field
      case _ => false)

  /** Primary-key order of `relation`, when the query reads the primary key. */
  def primaryKeyOrder(relation: BoundRelation, descending: Boolean): PhysicalProperties =
    PhysicalProperties(attributeOf(relation, relation.entity.primaryKey).map(a => SortOrder(a.slot, descending)).toVector)

  /** Whether an index on `index` holds every column `relation` reads. */
  def covers(relation: BoundRelation, index: IndexInfo): Boolean =
    relation.columns.forall(_.origin match
      case ColumnOrigin.Stored(_, _, field) => index.covers(field)
      case _ => true)
