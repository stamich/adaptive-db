package io.adb.model

/** Identifier of an entity (table); the high 64 bits of every storage key of its rows. */
opaque type EntityId = Long
/** Constructors and accessors of [[EntityId]]. */
object EntityId:
  /** Wraps a raw id. */
  def apply(value: Long): EntityId = value
  extension (id: EntityId) def value: Long = id

/** Identifier of a field (column) inside a row. */
opaque type FieldId = Int
/** Constructors and accessors of [[FieldId]]. */
object FieldId:
  /** Wraps a raw id. */
  def apply(value: Int): FieldId = value
  extension (id: FieldId) def value: Int = id

/** Version of a catalog schema. */
opaque type SchemaVersion = Long
/** Constructors and accessors of [[SchemaVersion]]. */
object SchemaVersion:
  /** Wraps a raw version. */
  def apply(value: Long): SchemaVersion = value
  extension (version: SchemaVersion) def value: Long = version

/** Field ids reserved by the engine; never part of a user schema. */
object SystemFields:
  val EntityIdField: FieldId = FieldId(0)
