package io.adb.statistics

import io.adb.model.*

/** How current an entity's statistics are. */
enum Freshness derives CanEqual:
  /** Few rows changed since `ANALYZE`. */
  case Fresh
  /** More than the stale threshold of the rows changed since `ANALYZE`; estimates still use
    * the statistics but with lower confidence, and EXPLAIN suggests running `ANALYZE` again.
    */
  case Stale

/** Statistics of one entity together with how much it changed since they were collected.
  *
  * @param table                     the statistics document
  * @param modificationsSinceAnalyze row mutations committed since `ANALYZE`
  */
final case class EntityStatistics(table: TableStatistics, modificationsSinceAnalyze: Long) derives CanEqual:
  /** Mutations since `ANALYZE` per analyzed row (an entity analyzed empty counts as one row). */
  def changeRatio: Double = modificationsSinceAnalyze.toDouble / math.max(table.rowCount, 1L).toDouble

  /** Fresh unless more than `staleThreshold` of the rows changed. */
  def freshness(staleThreshold: Double = EntityStatistics.DefaultStaleThreshold): Freshness =
    if changeRatio > staleThreshold then Freshness.Stale else Freshness.Fresh

  /** How far estimates built on these statistics can be trusted, in `(0, 1]`: 1.0 for an
    * exact fresh analysis, 0.9 for a sampled one, half of that once stale.
    */
  def confidence(staleThreshold: Double = EntityStatistics.DefaultStaleThreshold): Double =
    val base = if table.exact then 1.0 else 0.9
    if freshness(staleThreshold) == Freshness.Stale then base / 2 else base

/** Defaults of [[EntityStatistics]]. */
object EntityStatistics:
  /** Statistics are stale once more than 20% of the rows changed (PostgreSQL's autovacuum
    * analyze scale factor is 10%; planning here tolerates more because ANALYZE is manual).
    */
  val DefaultStaleThreshold: Double = 0.2

/** Source of optimizer statistics. Implementations must be cheap to call repeatedly while one
  * query is planned.
  */
trait StatisticsProvider:
  /** Statistics of `entity`, or `None` if it was never analyzed. */
  def statistics(entity: EntityId): Option[EntityStatistics]

/** Providers for planning without the engine. */
object StatisticsProvider:
  /** No statistics at all: every estimate falls back to defaults. */
  val Empty: StatisticsProvider = _ => None

  /** A fixed set of statistics (tests, benchmarks, what-if planning). */
  def of(entries: EntityStatistics*): StatisticsProvider =
    val byEntity = entries.map(e => e.table.entityId -> e).toMap
    entity => byEntity.get(entity)
