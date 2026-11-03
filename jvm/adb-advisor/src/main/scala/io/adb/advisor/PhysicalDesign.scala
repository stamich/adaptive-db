package io.adb.advisor

import io.adb.catalog.IndexInfo
import io.adb.statistics.EntityStatistics

/** What the advisor knows about one entity's physical design at the moment of an evaluation.
  *
  * Built by the caller from the catalog and the statistics provider; the advisor itself never
  * reads live structures.
  *
  * @param entityId   engine entity id (the same number [[io.adb.telemetry.FieldRef.entity]] carries)
  * @param name       entity name, used in recommendation texts and generated SQL
  * @param fieldNames column names by field id (the number [[io.adb.telemetry.FieldRef.field]] carries)
  * @param rowCount   row count of the last `ANALYZE` (0 if never analyzed)
  * @param statistics statistics of the entity, if it was ever analyzed
  * @param indexes    secondary indexes that exist now
  */
final case class EntityDesign(
    entityId: Long,
    name: String,
    fieldNames: Map[Long, String],
    rowCount: Long,
    statistics: Option[EntityStatistics],
    indexes: Vector[IndexInfo]
) derives CanEqual:
  /** Column name of `fieldId`, or `f<id>` when the schema no longer has it. */
  def fieldName(fieldId: Long): String = fieldNames.getOrElse(fieldId, s"f$fieldId")

  /** Whether some index of the entity has `fieldId` as its **first** key column, the only
    * position that serves an equality or range filter on that column alone.
    */
  def hasLeadingIndexOn(fieldId: Long): Boolean =
    indexes.exists(_.columns.headOption.exists(_.field.value.toLong == fieldId))

/** The physical design of the whole database: one [[EntityDesign]] per entity.
  *
  * @param entities designs by entity id
  */
final case class PhysicalDesign(entities: Map[Long, EntityDesign]) derives CanEqual:
  /** The design of `entityId`, if the entity exists. */
  def entity(entityId: Long): Option[EntityDesign] = entities.get(entityId)

/** Constants and constructors of [[PhysicalDesign]]. */
object PhysicalDesign:
  /** A design without any entity. */
  val empty: PhysicalDesign = PhysicalDesign(Map.empty)

  /** Builds a design from a list of entity designs. */
  def of(designs: Seq[EntityDesign]): PhysicalDesign = PhysicalDesign(designs.map(d => d.entityId -> d).toMap)
