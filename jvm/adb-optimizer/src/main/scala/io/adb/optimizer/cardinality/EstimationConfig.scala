package io.adb.optimizer.cardinality

import io.adb.statistics.EntityStatistics

/** Defaults the estimator falls back to when statistics do not answer a question.
  *
  * @param defaultRows                rows assumed for an entity that was never analyzed
  * @param defaultEqualitySelectivity selectivity of `column = value` without statistics
  * @param defaultRangeSelectivity    selectivity of a range or other comparison without statistics
  * @param defaultDistinct            distinct values assumed for a column without statistics
  * @param staleThreshold             fraction of changed rows above which statistics are stale
  */
final case class EstimationConfig(
    defaultRows: Double = 1000,
    defaultEqualitySelectivity: Double = 0.1,
    defaultRangeSelectivity: Double = 1.0 / 3,
    defaultDistinct: Double = 200,
    staleThreshold: Double = EntityStatistics.DefaultStaleThreshold
) derives CanEqual:
  require(defaultRows >= 1 && defaultDistinct >= 1, "default rows and distinct values must be at least 1")
  require(defaultEqualitySelectivity > 0 && defaultEqualitySelectivity <= 1, "invalid default equality selectivity")
  require(defaultRangeSelectivity > 0 && defaultRangeSelectivity <= 1, "invalid default range selectivity")
  require(staleThreshold >= 0, "stale threshold must be non-negative")
