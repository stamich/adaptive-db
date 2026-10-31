package io.adb.schema

import io.adb.model.*

/** One field of an entity across its whole life, as the engine catalog records it.
  *
  * Field ids are never reused: a dropped field keeps its id and stays in every later schema
  * version with `droppedIn` set, so rows written before the drop remain decodable and
  * `AS OF` queries of older snapshots still see it.
  *
  * @param id           stable field id (1, 2, 3, ... in declaration order; 0 is the system field)
  * @param name         current name (a rename changes only this)
  * @param dataType     logical type of the stored values
  * @param nullable     whether NULL is accepted
  * @param default      value of the field in rows written before it was added, and in new rows
  *                     that omit it (`None` = NULL)
  * @param introducedIn schema version that added the field
  * @param droppedIn    schema version that dropped it, if any
  * @param references   foreign-key hint (not enforced; an optimizer hint)
  */
final case class FieldDefinition(
    id: FieldId,
    name: String,
    dataType: DataType,
    nullable: Boolean,
    default: Option[DbValue],
    introducedIn: SchemaVersion,
    droppedIn: Option[SchemaVersion],
    references: Option[ForeignKeyRef]
) derives CanEqual:
  /** Whether the field exists in schema `version`: added at or before it and not dropped by then. */
  def isActiveIn(version: SchemaVersion): Boolean =
    introducedIn.value <= version.value && droppedIn.forall(_.value > version.value)

  /** Whether the field was dropped at some version. */
  def isDropped: Boolean = droppedIn.isDefined

/** One version of one entity's schema, as the engine catalog records it
  * (`adb_list_schemas_json`, `adb_schema_history_json`).
  *
  * @param entityId    engine entity id
  * @param name        entity (table) name
  * @param version     schema version, 1 for a new entity and +1 per change
  * @param parent      version this one was derived from (`None` for version 1)
  * @param epoch       catalog epoch at which this version was published
  * @param primaryKey  field id of the BIGINT primary key (never dropped or retyped)
  * @param nextFieldId id the next added field receives
  * @param fields      every field ever defined, in id order, dropped ones included
  */
final case class EntitySchema(
    entityId: EntityId,
    name: String,
    version: SchemaVersion,
    parent: Option[SchemaVersion],
    epoch: SchemaEpoch,
    primaryKey: FieldId,
    nextFieldId: FieldId,
    fields: Vector[FieldDefinition]
) derives CanEqual:
  /** Fields that exist in this version, in id order. */
  def activeFields: Vector[FieldDefinition] = fields.filter(_.isActiveIn(version))

  /** The active field named `name` (case-insensitive). */
  def field(name: String): Option[FieldDefinition] =
    activeFields.find(_.name.equalsIgnoreCase(name))

  /** The field with id `id`, active or dropped. */
  def fieldById(id: FieldId): Option[FieldDefinition] = fields.find(_.id == id)

  /** The primary-key field. */
  def primaryKeyField: FieldDefinition =
    fieldById(primaryKey).getOrElse(throw new IllegalStateException(s"entity $name has no primary key field"))

  /** The active columns as SQL column definitions, e.g.
    * `id BIGINT NOT NULL PRIMARY KEY, currency STRING NOT NULL DEFAULT 'EUR'` (`SHOW SCHEMA`).
    */
  def columnsSql: String =
    activeFields.map { field =>
      val notNull = if field.nullable then "" else " NOT NULL"
      val pk = if field.id == primaryKey then " PRIMARY KEY" else ""
      val default = field.default.fold("")(value => s" DEFAULT ${SchemaJson.sqlLiteral(value)}")
      s"${field.name} ${field.dataType.sqlName}$notNull$pk$default"
    }.mkString(", ")

  /** The binder's view of this version: active fields only, with their defaults and lifetimes. */
  def toEntity: Entity =
    Entity(
      entityId,
      name,
      activeFields.map(field =>
        Field(
          field.id,
          field.name,
          field.dataType,
          field.nullable,
          field.references,
          default = field.default,
          introducedIn = field.introducedIn
        )
      ),
      primaryKey,
      version
    )

/** A field of a `CREATE TABLE` statement or of an `ADD COLUMN` change.
  *
  * @param name       field name
  * @param dataType   logical type
  * @param nullable   whether NULL is accepted
  * @param default    default value (required for an added `NOT NULL` field)
  * @param references foreign-key hint
  * @param id         explicit field id (only when importing an existing catalog)
  */
final case class NewField(
    name: String,
    dataType: DataType,
    nullable: Boolean,
    default: Option[DbValue] = None,
    references: Option[ForeignKeyRef] = None,
    id: Option[FieldId] = None
) derives CanEqual

/** A `CREATE TABLE` definition sent to the engine (`adb_create_entity_json`).
  *
  * @param name       table name
  * @param fields     fields in declaration order (ids 1, 2, 3, ... unless given explicitly)
  * @param primaryKey name of the BIGINT, non-nullable primary-key field
  * @param entityId   explicit entity id (only when importing an existing catalog)
  */
final case class EntitySpec(
    name: String,
    fields: Vector[NewField],
    primaryKey: String,
    entityId: Option[EntityId] = None
) derives CanEqual
