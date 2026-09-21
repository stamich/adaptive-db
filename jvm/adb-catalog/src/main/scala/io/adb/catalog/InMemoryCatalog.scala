package io.adb.catalog

import io.adb.model.*

/** Documents `InMemoryCatalog` and its role in the Milestone 2.0.1 JVM control plane. */
final class InMemoryCatalog extends Catalog:
  private var currentVersion: Long = 0L
  private var nextEntityId: Long = 1L
  private var byId = Map.empty[Long, Entity]
  private var byName = Map.empty[String, Entity]

  /** Documents `version` and its role in the Milestone 2.0.1 JVM control plane. */
  override def version: SchemaVersion = SchemaVersion(currentVersion)
  /** Documents `entity` and its role in the Milestone 2.0.1 JVM control plane. */
  override def entity(name: String): Option[Entity] = byName.get(name.toLowerCase)
  /** Documents `entity` and its role in the Milestone 2.0.1 JVM control plane. */
  override def entity(id: EntityId): Option[Entity] = byId.get(id.value)
  /** Documents `entities` and its role in the Milestone 2.0.1 JVM control plane. */
  override def entities: Vector[Entity] = byId.values.toVector.sortBy(_.id.value)

  /** Documents `createEntity` and its role in the Milestone 2.0.1 JVM control plane. */
  override def createEntity(
      name: String,
      fields: Vector[(String, DataType, Boolean)],
      primaryKey: String
  ): Entity = synchronized {
    require(name.nonEmpty && name.length <= FileCatalog.MaxNameChars, s"entity name length must be 1..${FileCatalog.MaxNameChars}")
    require(!byName.contains(name.toLowerCase), s"entity already exists: $name")
    require(fields.nonEmpty, "entity must have at least one field")
    require(fields.size <= FileCatalog.MaxFieldsPerEntity, s"entity exceeds ${FileCatalog.MaxFieldsPerEntity} fields")
    require(fields.forall { case (n, _, _) => n.nonEmpty && n.length <= FileCatalog.MaxNameChars }, "invalid field name length")

    val duplicates = fields.groupBy(_._1.toLowerCase).collect { case (n, xs) if xs.size > 1 => n }
    require(duplicates.isEmpty, s"duplicate fields: ${duplicates.mkString(",")}")

    val materialized = fields.zipWithIndex.map { case ((fieldName, tpe, nullable), index) =>
      Field(FieldId(index + 1), fieldName, tpe, nullable)
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

  /** Documents `restore` and its role in the Milestone 2.0.1 JVM control plane. */
  private[catalog] def restore(version0: Long, nextId0: Long, restored: Vector[Entity]): Unit = synchronized {
    currentVersion = version0
    nextEntityId = nextId0
    byId = restored.map(e => e.id.value -> e).toMap
    byName = restored.map(e => e.name.toLowerCase -> e).toMap
  }
