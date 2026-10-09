package io.adb.catalog

import io.adb.model.*

/** Registry of entity schemas used to bind SQL statements. */
trait Catalog:
  /** Monotonic catalog version; incremented by every schema change. */
  def version: SchemaVersion
  /** Looks up an entity by name, case-insensitively. */
  def entity(name: String): Option[Entity]
  /** Looks up an entity by engine id. */
  def entity(id: EntityId): Option[Entity]
  /** All entities, ordered by id. */
  def entities: Vector[Entity]
  /** Creates an entity and bumps the catalog version.
    *
    * @param name       entity name (unique, case-insensitive)
    * @param fields     `(name, type, nullable)` per column, in declaration order
    * @param primaryKey name of the BIGINT, non-nullable primary-key column
    * @param references foreign-key hints by column name; each must name the primary key of an
    *                   existing entity, with the same type as the column
    * @return the created entity
    * @throws IllegalArgumentException if the definition is invalid or the name is taken
    */
  def createEntity(
      name: String,
      fields: Vector[(String, DataType, Boolean)],
      primaryKey: String,
      references: Map[String, ForeignKeyRef] = Map.empty
  ): Entity
