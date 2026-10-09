package io.adb.gateway

import io.adb.ffm.NativeDatabase
import io.adb.model.EntityId
import io.adb.statistics.*
import java.util.concurrent.ConcurrentHashMap
import scala.util.control.NonFatal

/** Statistics served by the native engine (`adb_statistics_json`, `adb_statistics_generation`,
  * `adb_modifications_since_analyze`).
  *
  * Decoded documents are cached by their **generation**, which the engine changes whenever any
  * client publishes a new document. Each call costs two cheap native calls (generation and
  * modification count; the estimator asks once per entity and query), plus one fetch and decode
  * after an `ANALYZE` from anywhere. A document that cannot be decoded counts as missing: a query
  * never fails because of statistics.
  *
  * @param document      the engine's document of an entity, if analyzed
  * @param generation    generation of an entity's document (0 if there is none)
  * @param modifications mutations of an entity since its last `ANALYZE`
  */
final class EngineStatisticsProvider(
    document: Long => Option[String],
    generation: Long => Long,
    modifications: Long => Long
) extends StatisticsProvider:
  /** Decoded documents with the generation they were decoded at, by entity id. */
  private val cache = new ConcurrentHashMap[Long, (Long, Option[TableStatistics])]()

  /** The current document (re-read when its generation changed) plus the modification count. */
  def statistics(entity: EntityId): Option[EntityStatistics] =
    val current = generation(entity.value)
    if current == 0 then None
    else
      val cached = cache.get(entity.value)
      val table =
        if cached != null && cached._1 == current then cached._2
        else
          val decoded =
            try document(entity.value).map(StatisticsCodec.decode)
            catch case NonFatal(_) => None
          cache.put(entity.value, current -> decoded)
          decoded
      table.map(t => EntityStatistics(t, modifications(entity.value)))

/** Construction from a native database. */
object EngineStatisticsProvider:
  /** A provider reading `native`. */
  def apply(native: NativeDatabase): EngineStatisticsProvider =
    new EngineStatisticsProvider(
      id => Option(native.statisticsJson(id).orElse(null)),
      native.statisticsGeneration,
      native.modificationsSinceAnalyze
    )
