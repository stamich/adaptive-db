package io.adb.model

/** Documents `Field` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Field(
    id: FieldId,
    name: String,
    dataType: DataType,
    nullable: Boolean
) derives CanEqual

/** Documents `Entity` and its role in the Milestone 2.0.1 JVM control plane. */
final case class Entity(
    id: EntityId,
    name: String,
    fields: Vector[Field],
    primaryKey: FieldId,
    schemaVersion: SchemaVersion
) derives CanEqual:
  lazy val fieldsByName: Map[String, Field] =
    fields.map(field => field.name.toLowerCase -> field).toMap

  lazy val fieldsById: Map[FieldId, Field] =
    fields.map(field => field.id -> field).toMap

  /** Documents `field` and its role in the Milestone 2.0.1 JVM control plane. */
  def field(name: String): Option[Field] = fieldsByName.get(name.toLowerCase)
  /** Documents `primaryKeyField` and its role in the Milestone 2.0.1 JVM control plane. */
  def primaryKeyField: Field = fieldsById(primaryKey)
