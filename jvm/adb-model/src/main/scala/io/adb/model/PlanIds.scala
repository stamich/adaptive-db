package io.adb.model

/** Identifier of one relation *instance* in a query (one FROM or JOIN item).
  *
  * A self-join reads the same entity twice; the two instances share an [[EntityId]] and every
  * [[FieldId]], but get different relation ids and therefore different slots.
  */
opaque type RelationId = Int
/** Constructors and accessors of [[RelationId]]. */
object RelationId:
  /** Wraps a raw id. */
  def apply(value: Int): RelationId = value
  extension (id: RelationId) def value: Int = id

/** Query-scoped column identifier: the column identity used by every native operator.
  *
  * Slots are allocated densely by the binder (`0, 1, 2, ...` in order of first reference), so a
  * native row is simply an array indexed by slot. The engine accepts slots below [[SlotId.MaxSlots]].
  */
opaque type SlotId = Int
/** Constructors, accessors and limits of [[SlotId]]. */
object SlotId:
  /** Slots per query accepted by the native engine. */
  val MaxSlots: Int = 4096
  /** Wraps a raw id. */
  def apply(value: Int): SlotId = value
  extension (id: SlotId) def value: Int = id

/** Aggregate functions understood by the planner and the native engine. */
enum AggregateFunction derives CanEqual:
  /** `COUNT(*)` (no input) or `COUNT(x)` (non-NULL values); BIGINT. */
  case Count
  /** Sum of BIGINT (exact, overflow is an error) or DOUBLE values. */
  case Sum
  /** Smallest value. */
  case Min
  /** Largest value. */
  case Max
  /** Arithmetic mean; DOUBLE. */
  case Avg

  /** SQL spelling, e.g. `SUM`. */
  def sqlName: String = toString.toUpperCase

/** Where the values of an [[Attribute]] come from. */
enum ColumnOrigin derives CanEqual:
  /** A stored field of one relation instance.
    *
    * @param relation relation instance the value is read from
    * @param entity   entity of that relation
    * @param field    stored field
    */
  case Stored(relation: RelationId, entity: EntityId, field: FieldId)
  /** A value computed by the query (an aggregate result).
    *
    * @param description human-readable definition, e.g. `SUM(o.amount)`
    */
  case Computed(description: String)

/** A typed column flowing through a plan.
  *
  * Logical operators exchange attributes; the physical planner only needs their slots.
  *
  * @param slot      query-scoped slot the value lives in
  * @param name      display name (`alias.column` for stored columns)
  * @param dataType  logical type
  * @param nullable  whether the value may be NULL (stored nullability, outer joins and aggregates)
  * @param origin    where the value comes from
  */
final case class Attribute(
    slot: SlotId,
    name: String,
    dataType: DataType,
    nullable: Boolean,
    origin: ColumnOrigin
) derives CanEqual:
  /** `name#slot`, the notation used by EXPLAIN. */
  def display: String = s"$name#${slot.value}"
