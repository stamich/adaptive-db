package io.adb.model

/** A declared, **not enforced** foreign key: the field holds primary keys of `entity`.
  *
  * The engine never checks it. It is an optimizer hint: a join `child.fk = parent.pk` matches
  * at most one parent row per child row, which pins the join's cardinality far better than
  * distinct-value counts can.
  *
  * @param entity referenced entity
  * @param field  referenced field (always that entity's primary key)
  */
final case class ForeignKeyRef(entity: EntityId, field: FieldId) derives CanEqual

/** One column of an entity.
  *
  * @param id         stable engine field identifier (1-based position at creation)
  * @param name       column name as declared
  * @param dataType   logical column type
  * @param nullable   whether the column accepts NULL
  * @param references declared foreign-key hint (`REFERENCES t(c) NOT ENFORCED`), if any
  */
final case class Field(
    id: FieldId,
    name: String,
    dataType: DataType,
    nullable: Boolean,
    references: Option[ForeignKeyRef] = None
) derives CanEqual

/** Schema of one entity (table).
  *
  * @param id            engine entity identifier; the high 64 bits of every row key
  * @param name          entity name as declared
  * @param fields        columns in declaration order
  * @param primaryKey    field id of the BIGINT primary-key column
  * @param schemaVersion catalog version that created this entity
  */
final case class Entity(
    id: EntityId,
    name: String,
    fields: Vector[Field],
    primaryKey: FieldId,
    schemaVersion: SchemaVersion
) derives CanEqual:
  /** Columns indexed by lower-cased name, for case-insensitive lookup. */
  lazy val fieldsByName: Map[String, Field] =
    fields.map(field => field.name.toLowerCase -> field).toMap

  /** Columns indexed by field id. */
  lazy val fieldsById: Map[FieldId, Field] =
    fields.map(field => field.id -> field).toMap

  /** Looks up a column by name, case-insensitively.
    *
    * @param name column name
    * @return the column, if it exists
    */
  def field(name: String): Option[Field] = fieldsByName.get(name.toLowerCase)
  /** The primary-key column. */
  def primaryKeyField: Field = fieldsById(primaryKey)
