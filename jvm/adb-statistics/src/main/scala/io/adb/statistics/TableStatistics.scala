package io.adb.statistics

import io.adb.model.*

/** Optimizer statistics of one entity, as collected by the native `ANALYZE`
  * (see docs/statistics.md). The engine owns and persists them; the JVM only reads them.
  *
  * @param entityId               entity described
  * @param rowCount               rows visible at the analyzed snapshot (exact)
  * @param avgRowBytes            average encoded row size
  * @param analyzedAtTs           commit timestamp of the analyzed snapshot
  * @param modificationsAtAnalyze the entity's modification counter at that snapshot
  * @param sampledRows            rows in the sample histograms and common values come from
  * @param exact                  whether the sample held every row
  * @param columns                per-field statistics
  * @param indexes                per-index statistics (statistics format 2; empty in format 1)
  */
final case class TableStatistics(
    entityId: EntityId,
    rowCount: Long,
    avgRowBytes: Double,
    analyzedAtTs: Long,
    modificationsAtAnalyze: Long,
    sampledRows: Long,
    exact: Boolean,
    columns: Map[FieldId, ColumnStatistics],
    indexes: Map[IndexId, IndexStatistics] = Map.empty
) derives CanEqual:
  /** Statistics of one field, if it was analyzed. */
  def column(field: FieldId): Option[ColumnStatistics] = columns.get(field)

  /** Statistics of one index, if it existed when the entity was analyzed. */
  def index(id: IndexId): Option[IndexStatistics] = indexes.get(id)

/** Statistics of one field.
  *
  * @param fieldId       field described
  * @param nullCount     rows where the field is NULL or absent (exact)
  * @param distinctCount distinct non-NULL values (exact up to 10,000, HyperLogLog above)
  * @param distinctExact whether `distinctCount` is exact
  * @param min           smallest value (absent for empty or mixed-type columns)
  * @param max           largest value
  * @param avgWidthBytes average width of the non-NULL values
  * @param histogram     equi-depth buckets in ascending value order
  * @param mostCommon    markedly frequent values, most frequent first
  */
final case class ColumnStatistics(
    fieldId: FieldId,
    nullCount: Long,
    distinctCount: Long,
    distinctExact: Boolean,
    min: Option[DbValue],
    max: Option[DbValue],
    avgWidthBytes: Double,
    histogram: Vector[HistogramBucket],
    mostCommon: Vector[MostCommonValue]
) derives CanEqual:
  /** Rows covered by the most common values. */
  def mostCommonRows: Long = mostCommon.iterator.map(_.rows).sum

/** One equi-depth histogram bucket: `rows` values in `[lower, upper]`, `distinct` of them distinct. */
final case class HistogramBucket(lower: DbValue, upper: DbValue, rows: Long, distinct: Long) derives CanEqual

/** A frequent value and the number of rows holding it (scaled from the sample). */
final case class MostCommonValue(value: DbValue, rows: Long) derives CanEqual

/** Statistics of one secondary index, collected by the same `ANALYZE` as its table.
  *
  * @param indexId          index described
  * @param entries          entries visible at the analyzed snapshot (one per live row)
  * @param storedEntries    entries stored, including ended versions kept for older snapshots
  * @param height           levels from the root to the leaves
  * @param leafPages        leaf pages of the index tree
  * @param avgKeyBytes      mean encoded key size
  * @param distinctPrefixes element `i`: distinct combinations of the first `i + 1` key columns
  * @param pkLocality       fraction of consecutive entries whose rows are storage neighbours, in
  *                         `[0, 1]`; fetching rows in index order is sequential near 1, random near 0
  */
final case class IndexStatistics(
    indexId: IndexId,
    entries: Long,
    storedEntries: Long,
    height: Int,
    leafPages: Long,
    avgKeyBytes: Double,
    distinctPrefixes: Vector[Long],
    pkLocality: Double
) derives CanEqual:
  /** Distinct combinations of the first `length` key columns, if known (`length >= 1`). */
  def distinctPrefix(length: Int): Option[Long] = distinctPrefixes.lift(length - 1)

