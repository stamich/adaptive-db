package io.adb.catalog

import io.adb.model.*

/** Thread-safe, non-persistent [[Catalog]]; also the in-memory state behind [[FileCatalog]]. */
final class InMemoryCatalog extends Catalog:
  /** Current catalog version. */
  private var currentVersion: Long = 0L
  /** Id assigned to the next created entity. */
  private var nextEntityId: Long = 1L
  /** Entities by id. */
  private var byId = Map.empty[Long, Entity]
  /** Entities by lower-cased name. */
  private var byName = Map.empty[String, Entity]

  /** Current catalog version. */
  override def version: SchemaVersion = SchemaVersion(currentVersion)
  /** Looks up an entity by lower-cased name. */
  override def entity(name: String): Option[Entity] = byName.get(name.toLowerCase)
  /** Looks up an entity by id. */
  override def entity(id: EntityId): Option[Entity] = byId.get(id.value)
  /** All entities sorted by id. */
  override def entities: Vector[Entity] = byId.values.toVector.sortBy(_.id.value)

  /** Validates the definition (name lengths, unique columns, BIGINT non-null primary key,
    * foreign-key hints that name the primary key of an existing entity with the same type),
    * assigns field ids by position, and registers the entity.
    */
  override def createEntity(
      name: String,
      fields: Vector[(String, DataType, Boolean)],
      primaryKey: String,
      references: Map[String, ForeignKeyRef] = Map.empty
  ): Entity = synchronized {
    require(name.nonEmpty && name.length <= FileCatalog.MaxNameChars, s"entity name length must be 1..${FileCatalog.MaxNameChars}")
    require(!byName.contains(name.toLowerCase), s"entity already exists: $name")
    require(fields.nonEmpty, "entity must have at least one field")
    require(fields.size <= FileCatalog.MaxFieldsPerEntity, s"entity exceeds ${FileCatalog.MaxFieldsPerEntity} fields")
    require(fields.forall { case (n, _, _) => n.nonEmpty && n.length <= FileCatalog.MaxNameChars }, "invalid field name length")

    val duplicates = fields.groupBy(_._1.toLowerCase).collect { case (n, xs) if xs.size > 1 => n }
    require(duplicates.isEmpty, s"duplicate fields: ${duplicates.mkString(",")}")

    val referencesByName = references.map((column, target) => column.toLowerCase -> target)
    require(referencesByName.size == references.size, "duplicate foreign-key columns")
    referencesByName.keys.foreach { column =>
      require(fields.exists(_._1.equalsIgnoreCase(column)), s"foreign key on unknown column $column")
    }
    val materialized = fields.zipWithIndex.map { case ((fieldName, tpe, nullable), index) =>
      val reference = referencesByName.get(fieldName.toLowerCase)
      reference.foreach(target => InMemoryCatalog.validateReference(byId, fieldName, tpe, target))
      Field(FieldId(index + 1), fieldName, tpe, nullable, reference)
    }
    val pk = materialized.find(_.name.equalsIgnoreCase(primaryKey))
      .getOrElse(throw new IllegalArgumentException(s"unknown primary key: $primaryKey"))
    require(pk.dataType == DataType.Int64, "Milestone 2 primary key must be BIGINT")
    require(!pk.nullable, "primary key cannot be nullable")

    if currentVersion == Long.MaxValue || nextEntityId == Long.MaxValue then
      throw new IllegalStateException("catalog identifier/version space exhausted")
    currentVersion += 1
    val entity = Entity(
      EntityId(nextEntityId),
      name,
      materialized,
      pk.id,
      SchemaVersion(currentVersion)
    )
    nextEntityId += 1
    byId += entity.id.value -> entity
    byName += name.toLowerCase -> entity
    entity
  }

  /** Replaces the whole state with a snapshot loaded from disk (used by [[FileCatalog]]).
    *
    * @param version0 catalog version to restore
    * @param nextId0  next entity id to restore
    * @param restored entities to restore
    */
  private[catalog] def restore(version0: Long, nextId0: Long, restored: Vector[Entity]): Unit = synchronized {
    currentVersion = version0
    nextEntityId = nextId0
    byId = restored.map(e => e.id.value -> e).toMap
    byName = restored.map(e => e.name.toLowerCase -> e).toMap
  }

/** Validation shared by [[InMemoryCatalog]] and [[FileCatalog]]. */
private[catalog] object InMemoryCatalog:
  /** Checks that a foreign-key hint on `column` names the primary key of an entity in
    * `entities` and has the same type.
    *
    * @throws IllegalArgumentException otherwise
    */
  def validateReference(entities: Map[Long, Entity], column: String, dataType: DataType, target: ForeignKeyRef): Unit =
    val parent = entities.getOrElse(
      target.entity.value,
      throw new IllegalArgumentException(s"$column references unknown entity ${target.entity.value}")
    )
    require(target.field == parent.primaryKey, s"$column must reference the primary key of ${parent.name}")
    require(
      parent.primaryKeyField.dataType == dataType,
      s"$column is ${dataType.sqlName} but ${parent.name}.${parent.primaryKeyField.name} is ${parent.primaryKeyField.dataType.sqlName}"
    )
