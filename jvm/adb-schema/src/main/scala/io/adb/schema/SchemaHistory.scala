package io.adb.schema

import io.adb.model.*

/** Every schema version of one entity, oldest first (`adb_schema_history_json`).
  *
  * Versions form a chain: each has the previous one as parent. The history answers which schema
  * was current at a catalog epoch (what an `AS OF` query sees) and reconstructs the change that
  * produced each version (`SHOW SCHEMA`).
  *
  * @param entityId entity described
  * @param versions versions 1, 2, 3, ... in order
  */
final case class SchemaHistory(entityId: EntityId, versions: Vector[EntitySchema]) derives CanEqual:
  require(versions.nonEmpty, s"entity ${entityId.value} has an empty schema history")
  require(
    versions.zipWithIndex.forall((schema, index) => schema.version.value == index + 1L && schema.entityId == entityId),
    s"schema history of entity ${entityId.value} is not the chain 1, 2, 3, ..."
  )

  /** The latest version. */
  def latest: EntitySchema = versions.last

  /** Version `version`, if it exists. */
  def at(version: SchemaVersion): Option[EntitySchema] =
    versions.lift((version.value - 1).toInt).filter(_ => version.value >= 1)

  /** The version current at catalog epoch `epoch`: the newest published at or before it
    * (`None` if the entity did not exist yet).
    */
  def atEpoch(epoch: SchemaEpoch): Option[EntitySchema] =
    versions.takeWhile(_.epoch.value <= epoch.value).lastOption

  /** The change that turned each version's parent into it, paired with the version it produced
    * (version 1, the creation, has no change).
    */
  def changes: Vector[(SchemaVersion, SchemaChange)] =
    versions.sliding(2).collect { case Vector(before, after) => after.version -> SchemaHistory.diff(before, after) }.toVector

/** Reconstruction of changes from consecutive versions. */
object SchemaHistory:
  /** The single change between two consecutive versions of an entity.
    *
    * @throws IllegalArgumentException if `after` does not follow `before` by exactly one change
    */
  def diff(before: EntitySchema, after: EntitySchema): SchemaChange =
    require(after.parent.contains(before.version), s"version ${after.version.value} does not follow ${before.version.value}")
    val added = after.fields.filter(field => before.fieldById(field.id).isEmpty)
    val dropped = after.fields.filter(field => field.isDropped && before.fieldById(field.id).exists(!_.isDropped))
    val renamed = after.fields.flatMap(field => before.fieldById(field.id).filter(_.name != field.name).map(_ -> field))
    (added, dropped, renamed) match
      case (Vector(field), Vector(), Vector()) =>
        SchemaChange.AddField(NewField(field.name, field.dataType, field.nullable, field.default, field.references))
      case (Vector(), Vector(field), Vector()) => SchemaChange.DropField(field.name)
      case (Vector(), Vector(), Vector((from, to))) => SchemaChange.RenameField(from.name, to.name)
      case _ => throw new IllegalArgumentException(s"versions ${before.version.value} and ${after.version.value} differ by more than one change")
