package io.adb.statistics

import io.adb.model.*
import java.nio.charset.StandardCharsets

/** Statistics seen through the entity's current schema (Milestone 2.4.0).
  *
  * `ANALYZE` reads rows at the schema version current at that moment and records it
  * (`TableStatistics.schemaVersion`). A column added later has no statistics of its own, but its
  * values in the analyzed rows are known exactly: every row stored before the column existed
  * reads the column's default ("virtual default"). This object derives that column's statistics
  * from the default, so a predicate on a freshly added column is estimated as precisely as
  * the rest of the table instead of falling back to default selectivities. Rows written since
  * `ANALYZE` are covered by the usual staleness rules (they count as modifications).
  */
object SchemaStatistics:
  /** A column of the current schema that was added after the entity was analyzed.
    *
    * @param field       the column (with its default and the version that added it)
    * @param analyzedAt  schema version the statistics were collected at
    */
  final case class AddedAfterAnalyze(field: Field, analyzedAt: Long) derives CanEqual:
    /** EXPLAIN text: what the estimate of this column is based on. */
    def display: String =
      val basis = field.default.fold("every analyzed row reads NULL")(value => s"every analyzed row reads its DEFAULT ${literal(value)}")
      s"${field.name} added in version ${field.introducedIn.value}, after ANALYZE (version ${math.max(analyzedAt, 1L)}): $basis"

  /** The columns of `entity` added after `table` was analyzed that have no statistics. */
  def addedAfterAnalyze(table: TableStatistics, entity: Entity): Vector[AddedAfterAnalyze] =
    // Version 0 means the rows were read without a schema (or by a pre-2.4.0 ANALYZE): they
    // match version 1, the version every imported or created column starts at.
    val analyzedAt = math.max(table.schemaVersion, 1L)
    entity.fields.collect {
      case field if field.introducedIn.value > analyzedAt && table.column(field.id).isEmpty =>
        AddedAfterAnalyze(field, table.schemaVersion)
    }

  /** `statistics` extended with derived statistics for the columns of `entity` added after
    * `ANALYZE`; unchanged when there are none.
    */
  def adjust(statistics: EntityStatistics, entity: Entity): EntityStatistics =
    val added = addedAfterAnalyze(statistics.table, entity)
    if added.isEmpty then statistics
    else
      val rows = statistics.table.rowCount
      val derived = added.map(a => a.field.id -> derivedColumn(a.field, rows))
      statistics.copy(table = statistics.table.copy(columns = statistics.table.columns ++ derived))

  /** A provider that adjusts every entity's statistics to its current schema in `entities`. */
  def provider(base: StatisticsProvider, entities: EntityId => Option[Entity]): StatisticsProvider =
    id => base.statistics(id).map(stats => entities(id).fold(stats)(adjust(stats, _)))

  /** Statistics of a column whose value is `field.default` (or NULL) in all `rows` rows. */
  private def derivedColumn(field: Field, rows: Long): ColumnStatistics =
    field.default match
      case None | Some(DbValue.NullValue) =>
        ColumnStatistics(field.id, rows, 0, true, None, None, 0.0, Vector.empty, Vector.empty)
      case Some(value) =>
        ColumnStatistics(
          field.id,
          0,
          if rows == 0 then 0 else 1,
          true,
          Some(value),
          Some(value),
          width(value),
          Vector.empty,
          if rows == 0 then Vector.empty else Vector(MostCommonValue(value, rows))
        )

  /** Encoded width of a value, as `ANALYZE` measures it. */
  private def width(value: DbValue): Double = value match
    case DbValue.NullValue => 0.0
    case DbValue.BoolValue(_) => 1.0
    case DbValue.Int64Value(_) | DbValue.Float64Value(_) => 8.0
    case DbValue.StringValue(v) => v.getBytes(StandardCharsets.UTF_8).length.toDouble
    case DbValue.BytesValue(v) => v.length.toDouble

  /** A value as a SQL literal. */
  private def literal(value: DbValue): String = value match
    case DbValue.NullValue => "NULL"
    case DbValue.BoolValue(v) => if v then "TRUE" else "FALSE"
    case DbValue.Int64Value(v) => v.toString
    case DbValue.Float64Value(v) => v.toString
    case DbValue.StringValue(v) => "'" + v.replace("'", "''") + "'"
    case DbValue.BytesValue(v) => v.map(b => f"${b & 0xff}%02x").mkString("X'", "", "'")
