package io.adb.catalog

import io.adb.model.*
import io.adb.schema.*

/** One-time import of a pre-2.4.0 catalog (`catalog.properties`, see [[FileCatalog]]) into the
  * engine catalog.
  *
  * Before Milestone 2.4.0 the JVM owned the schemas; the engine only stored rows by entity and
  * field id. The import recreates every entity in the engine with the **same** entity and field
  * ids, so the stored rows match their schema without being rewritten: they carry no schema
  * version and are read as version 1. The import is idempotent: entities the engine already
  * knows (same id and name) are skipped, so an interrupted import completes on the next open.
  * Once it succeeded the caller retires the file (the gateway renames it to
  * `catalog.properties.imported`); from then on the engine catalog is the only one.
  */
object CatalogImport:
  /** What an import did.
    *
    * @param imported entities created in the engine
    * @param skipped  entities the engine already had
    */
  final case class Report(imported: Vector[Entity], skipped: Vector[Entity]) derives CanEqual

  /** The engine definition of a legacy entity: explicit entity id, field ids and hints. */
  def spec(entity: Entity): EntitySpec =
    EntitySpec(
      entity.name,
      entity.fields.map(field => NewField(field.name, field.dataType, field.nullable, field.default, field.references, Some(field.id))),
      entity.primaryKeyField.name,
      Some(entity.id)
    )

  /** Imports every entity of `legacy` that `target` does not have yet, parents before children
    * (ids ascend in legacy catalogs, and foreign-key hints only name existing entities).
    *
    * @throws IllegalStateException if an entity's id or name is taken by a different entity
    */
  def importInto(target: SchemaCatalog, legacy: Catalog): Report =
    target.ensureCurrent()
    val (skipped, missing) = legacy.entities.sortBy(_.id.value).partition { entity =>
      (target.entity(entity.id), target.entity(entity.name)) match
        case (Some(existing), _) if existing.name.equalsIgnoreCase(entity.name) => true
        case (None, None) => false
        case (byId, byName) =>
          val other = byId.orElse(byName).get
          throw new IllegalStateException(
            s"cannot import table ${entity.name} (id ${entity.id.value}): the engine has table ${other.name} (id ${other.id.value})"
          )
    }
    Report(missing.map(entity => target.createEntity(spec(entity))), skipped)
