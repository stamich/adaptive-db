package io.adb.gateway

import io.adb.ffm.NativeDatabase
import io.adb.model.EntityId
import io.adb.statistics.*
import java.util.concurrent.ConcurrentHashMap

/** Statistics served by the native engine (`adb_statistics_json`,
  * `adb_modifications_since_analyze`).
  *
  * Documents change only through `ANALYZE`, so they are decoded once and cached until
  * [[invalidate]]; the modification counter changes with every commit and is read on every
  * call (one cheap native call; the estimator asks once per entity and query).
  *
  * @param document      the engine's document of an entity, if analyzed
  * @param modifications mutations of an entity since its last `ANALYZE`
  */
final class EngineStatisticsProvider(document: Long => Option[String], modifications: Long => Long) extends StatisticsProvider:
  /** Decoded documents (`None` for entities never analyzed), by entity id. */
  private val cache = new ConcurrentHashMap[Long, Option[TableStatistics]]()

  /** The cached document plus the current modification count. */
  def statistics(entity: EntityId): Option[EntityStatistics] =
    val table = cache.computeIfAbsent(entity.value, id => document(id).map(StatisticsCodec.decode))
    table.map(t => EntityStatistics(t, modifications(entity.value)))

  /** Forgets the cached document of `entity` (after `ANALYZE`). */
  def invalidate(entity: EntityId): Unit = cache.remove(entity.value)

/** Construction from a native database. */
object EngineStatisticsProvider:
  /** A provider reading `native`. */
  def apply(native: NativeDatabase): EngineStatisticsProvider =
    new EngineStatisticsProvider(id => Option(native.statisticsJson(id).orElse(null)), native.modificationsSinceAnalyze)
