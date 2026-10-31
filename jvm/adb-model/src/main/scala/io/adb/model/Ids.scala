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

/** Identifier of a secondary index; assigned by the engine and never reused. */
opaque type IndexId = Long
/** Constructors and accessors of [[IndexId]]. */
object IndexId:
  /** Wraps a raw id. */
  def apply(value: Long): IndexId = value
  extension (id: IndexId) def value: Long = id

/** Version of a schema: of one entity in the engine catalog (1 = as created, +1 per `ALTER`),
  * or of the whole catalog in the file-based catalogs used before Milestone 2.4.0.
  */
opaque type SchemaVersion = Long
/** Constructors and accessors of [[SchemaVersion]]. */
object SchemaVersion:
  /** Wraps a raw version. */
  def apply(value: Long): SchemaVersion = value
  extension (version: SchemaVersion) def value: Long = version

/** Catalog epoch of the engine: advanced by every committed DDL statement (`CREATE`, `ALTER`,
  * `DROP TABLE`, `CREATE`/`DROP INDEX`). A physical plan records the epoch it was made at and the
  * engine refuses it once the epoch moved on (plan wire v4).
  */
opaque type SchemaEpoch = Long
/** Constructors and accessors of [[SchemaEpoch]]. */
object SchemaEpoch:
  /** Wraps a raw epoch. */
  def apply(value: Long): SchemaEpoch = value
  extension (epoch: SchemaEpoch) def value: Long = epoch

/** Field ids reserved by the engine; never part of a user schema. */
object SystemFields:
  val EntityIdField: FieldId = FieldId(0)
