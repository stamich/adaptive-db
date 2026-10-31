package io.adb.catalog

import io.adb.model.*
import io.adb.schema.*

/** The native engine's catalog functions (C ABI 7) as plain JSON calls, so this module needs no
  * dependency on the FFM bindings (`io.adb.ffm.NativeDatabase` implements them one to one).
  */
trait SchemaBackend:
  /** `adb_current_schema_epoch`. */
  def currentSchemaEpoch(): Long
  /** `adb_list_schemas_json`. */
  def listSchemasJson(): String
  /** `adb_create_entity_json`. */
  def createEntityJson(specJson: String): String
  /** `adb_alter_entity_json`. */
  def alterEntityJson(requestJson: String): String
  /** `adb_drop_entity`. */
  def dropEntity(entityId: Long): Unit
  /** `adb_schema_history_json`. */
  def schemaHistoryJson(entityId: Long): String

/** The catalog owned by the engine (Milestone 2.4.0): schemas live in the engine's log as
  * catalog records, so DDL is transactional and durable with the data, and every client sees
  * the same versions.
  *
  * The JVM keeps a cached copy of the latest schemas for binding. [[ensureCurrent]] compares the
  * cached epoch with the engine's (one cheap native call) and reloads the listing when they
  * differ; DDL issued through this object reloads it right away. A plan bound against a stale
  * copy is still safe: it carries its epoch and the engine refuses it (`SCHEMA_CHANGED`).
  *
  * Requests are checked by [[SchemaEvolutionPlanner]] first, for SQL-level messages without a
  * round trip; the engine re-checks everything.
  *
  * @param backend      the engine's catalog functions
  * @param dependencies the live indexes as dependency inputs (for RESTRICT messages)
  */
final class EngineCatalog(backend: SchemaBackend, dependencies: () => Vector[IndexDependency] = () => Vector.empty)
    extends SchemaCatalog:
  /** The cached listing; replaced as a whole, so readers never see a mix of two epochs. */
  @volatile private var listing: SchemaJson.CatalogListing = SchemaJson.decodeCatalog(backend.listSchemasJson())
  /** Binder views of the cached listing, by id and by lower-cased name. */
  @volatile private var views: EngineCatalog.Views = EngineCatalog.Views(listing)

  override def epoch: SchemaEpoch = listing.epoch
  /** The catalog epoch, as the generic catalog version. */
  override def version: SchemaVersion = SchemaVersion(listing.epoch.value)
  override def entity(name: String): Option[Entity] = views.byName.get(name.toLowerCase)
  override def entity(id: EntityId): Option[Entity] = views.byId.get(id)
  override def entities: Vector[Entity] = views.ordered
  override def schema(id: EntityId): Option[EntitySchema] = listing.entities.find(_.entityId == id)

  override def ensureCurrent(): SchemaEpoch = synchronized {
    if backend.currentSchemaEpoch() != listing.epoch.value then reload()
    listing.epoch
  }

  /** `CREATE TABLE` from column triples, as the 2.3.0 catalog API. */
  override def createEntity(
      name: String,
      fields: Vector[(String, DataType, Boolean)],
      primaryKey: String,
      references: Map[String, ForeignKeyRef] = Map.empty
  ): Entity =
    val byColumn = references.map((column, target) => column.toLowerCase -> target)
    createEntity(EntitySpec(name, fields.map((field, dataType, nullable) => NewField(field, dataType, nullable, references = byColumn.get(field.toLowerCase))), primaryKey))

  override def createEntity(spec: EntitySpec): Entity = synchronized {
    SchemaEvolutionPlanner.checkCreate(spec).foreach(reason => throw new IllegalArgumentException(reason))
    val created = SchemaJson.decodeSchema(backend.createEntityJson(SchemaJson.encodeSpec(spec)))
    reload()
    entity(created.entityId).getOrElse(created.toEntity)
  }

  override def alterEntity(id: EntityId, change: SchemaChange): EntitySchema = synchronized {
    ensureCurrent()
    val current = schema(id).getOrElse(throw new IllegalArgumentException(s"table ${id.value} does not exist"))
    val graph = DependencyGraph.build(listing.entities, dependencies())
    SchemaEvolutionPlanner.plan(current, change, graph, listing.epoch).left.foreach(reason => throw new IllegalArgumentException(reason))
    val changed = SchemaJson.decodeSchema(backend.alterEntityJson(SchemaJson.encodeAlter(id, change)))
    reload()
    changed
  }

  override def dropEntity(id: EntityId): EntitySchema = synchronized {
    ensureCurrent()
    val current = schema(id).getOrElse(throw new IllegalArgumentException(s"table ${id.value} does not exist"))
    DependencyGraph.build(listing.entities, dependencies())
      .dropRefusal(CatalogObjectId.Entity(id))
      .foreach(reason => throw new IllegalArgumentException(reason))
    backend.dropEntity(id.value)
    reload()
    current
  }

  override def history(id: EntityId): SchemaHistory = SchemaJson.decodeHistory(backend.schemaHistoryJson(id.value))

  /** Replaces the cached listing with the engine's current one. */
  private def reload(): Unit =
    val fresh = SchemaJson.decodeCatalog(backend.listSchemasJson())
    views = EngineCatalog.Views(fresh)
    listing = fresh

/** Helpers of [[EngineCatalog]]. */
object EngineCatalog:
  /** Binder views of one listing.
    *
    * @param ordered entities in id order
    * @param byId    entities by id
    * @param byName  entities by lower-cased name
    */
  private[catalog] final case class Views(ordered: Vector[Entity], byId: Map[EntityId, Entity], byName: Map[String, Entity])

  /** Views of a listing. */
  private[catalog] object Views:
    /** Builds the views of `listing`. */
    def apply(listing: SchemaJson.CatalogListing): Views =
      val ordered = listing.entities.sortBy(_.entityId.value).map(_.toEntity)
      Views(ordered, ordered.map(e => e.id -> e).toMap, ordered.map(e => e.name.toLowerCase -> e).toMap)
