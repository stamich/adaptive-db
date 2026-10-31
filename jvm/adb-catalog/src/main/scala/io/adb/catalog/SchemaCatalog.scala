package io.adb.catalog

import io.adb.model.*
import io.adb.schema.*

/** A [[Catalog]] whose schemas are versioned and can change: `ALTER TABLE`, `DROP TABLE` and the
  * version history behind `SHOW SCHEMA`. The engine catalog ([[EngineCatalog]]) is the only
  * implementation; the in-memory and file catalogs predate schema evolution and stay read-mostly
  * helpers for tests, tools and the import of 2.3.0 catalogs.
  */
trait SchemaCatalog extends Catalog:
  /** Catalog epoch of the schemas currently held (advanced by every committed DDL statement). */
  def epoch: SchemaEpoch

  /** The latest schema version of a live entity, with dropped fields and defaults. */
  def schema(id: EntityId): Option[EntitySchema]

  /** Creates an entity from a full definition (defaults, foreign-key hints, explicit ids). */
  def createEntity(spec: EntitySpec): Entity

  /** Applies one change to a live entity and returns the new version.
    *
    * @throws IllegalArgumentException if the entity does not exist or the change is refused
    */
  def alterEntity(id: EntityId, change: SchemaChange): EntitySchema

  /** Drops a live entity and returns its last version.
    *
    * @throws IllegalArgumentException if the entity does not exist or something depends on it
    */
  def dropEntity(id: EntityId): EntitySchema

  /** Every version of an entity, also of a dropped one. */
  def history(id: EntityId): SchemaHistory

  /** Makes the held schemas those of the current catalog epoch, reloading them if another
    * client changed the catalog; returns that epoch. Cheap when nothing changed.
    */
  def ensureCurrent(): SchemaEpoch
