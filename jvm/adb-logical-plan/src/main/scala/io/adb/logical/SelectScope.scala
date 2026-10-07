package io.adb.logical

import io.adb.model.*
import io.adb.sql.SqlExpr
import scala.collection.mutable

/** Name scope and slot allocator of one SELECT.
  *
  * Relations are registered in FROM order. A column gets its slot on *first reference*, so slots
  * are dense (`0, 1, 2, ...`), only referenced fields are ever read by the native scans, and the
  * same field of two relation instances (self-join) always gets two different slots:
  *
  * {{{
  * FROM account a JOIN account b ...   a.id -> #0, b.id -> #1   (both are field 1 of entity 7)
  * }}}
  */
private[logical] final class SelectScope:
  /** One registered relation instance and the attributes allocated for it so far. */
  final class RelationState(
      /** Query-scoped id (FROM position). */
      val id: RelationId,
      /** Entity read. */
      val entity: Entity,
      /** Name used by the query. */
      val alias: String,
      /** True for the right side of a LEFT JOIN: its values may be NULL-filled. */
      val outer: Boolean
  ):
    /** Allocated attributes by field, in allocation order. */
    val columns: mutable.LinkedHashMap[FieldId, Attribute] = mutable.LinkedHashMap.empty

    /** The immutable relation with every allocated column, in slot order. */
    def bound: BoundRelation = BoundRelation(id, entity, alias, columns.values.toVector.sortBy(_.slot.value))

  /** Registered relations, in FROM order. */
  private val relations = mutable.ArrayBuffer.empty[RelationState]
  /** Next free slot. */
  private var nextSlot = 0

  /** Registers a relation instance.
    *
    * @throws IllegalArgumentException if the alias is already used in this query
    */
  def addRelation(entity: Entity, alias: String, outer: Boolean): RelationState =
    if relations.exists(_.alias.equalsIgnoreCase(alias)) then
      throw new IllegalArgumentException(s"table name or alias '$alias' is specified more than once")
    val state = RelationState(RelationId(relations.size), entity, alias, outer)
    relations += state
    state

  /** Every registered relation, in FROM order. */
  def all: Vector[RelationState] = relations.toVector

  /** The attribute of `field` in `relation`, allocating its slot on first use. */
  def attribute(relation: RelationState, field: Field): Attribute =
    relation.columns.getOrElseUpdate(
      field.id,
      Attribute(
        allocate(),
        s"${relation.alias}.${field.name}",
        field.dataType,
        field.nullable || relation.outer,
        ColumnOrigin.Stored(relation.id, relation.entity.id, field.id)
      )
    )

  /** A fresh slot for a value computed by the query (aggregate results). */
  def computed(name: String, dataType: DataType, nullable: Boolean): Attribute =
    Attribute(allocate(), name, dataType, nullable, ColumnOrigin.Computed(name))

  /** Resolves a column reference against the first `visible` relations.
    *
    * A qualified reference names its relation by alias; an unqualified one must match exactly
    * one visible relation.
    *
    * @throws IllegalArgumentException for unknown tables or columns and ambiguous references
    */
  def resolve(column: SqlExpr.Column, visible: Int): Attribute =
    val inScope = relations.take(visible)
    column.qualifier match
      case Some(qualifier) =>
        val relation = inScope.find(_.alias.equalsIgnoreCase(qualifier)).getOrElse {
          val known = relations.exists(_.alias.equalsIgnoreCase(qualifier))
          throw new IllegalArgumentException(
            if known then s"table '$qualifier' is not visible here (ON may only use tables joined so far)"
            else s"unknown table or alias '$qualifier'"
          )
        }
        val field = relation.entity.field(column.name).getOrElse(
          throw new IllegalArgumentException(s"unknown column ${relation.alias}.${column.name}")
        )
        attribute(relation, field)
      case None =>
        val matches = inScope.flatMap(relation => relation.entity.field(column.name).map(relation -> _))
        matches match
          case mutable.ArrayBuffer((relation, field)) => attribute(relation, field)
          case found if found.isEmpty => throw new IllegalArgumentException(s"unknown column ${column.name}")
          case found =>
            throw new IllegalArgumentException(
              s"column reference '${column.name}' is ambiguous (${found.map((r, f) => s"${r.alias}.${f.name}").mkString(", ")})"
            )

  /** Hands out the next slot.
    *
    * @throws IllegalArgumentException once the native slot limit is reached
    */
  private def allocate(): SlotId =
    if nextSlot >= SlotId.MaxSlots then
      throw new IllegalArgumentException(s"query references more than ${SlotId.MaxSlots} columns")
    val slot = SlotId(nextSlot)
    nextSlot += 1
    slot
