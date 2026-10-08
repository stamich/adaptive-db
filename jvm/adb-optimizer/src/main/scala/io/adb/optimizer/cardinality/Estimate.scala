package io.adb.optimizer.cardinality

/** What an estimate rests on, from weakest to strongest. An operator's estimate inherits the
  * weakest source among its inputs.
  */
enum EstimateSource(val label: String) derives CanEqual:
  /** No statistics: built-in defaults. */
  case Default extends EstimateSource("defaults (no statistics)")
  /** Statistics whose entity changed by more than the stale threshold since `ANALYZE`. */
  case Stale extends EstimateSource("stale statistics")
  /** Fresh statistics. */
  case Statistics extends EstimateSource("statistics")

/** Companion helpers of [[EstimateSource]]. */
object EstimateSource:
  /** The weaker of two sources. */
  def weakest(a: EstimateSource, b: EstimateSource): EstimateSource = if a.ordinal <= b.ordinal then a else b

/** Estimated output of one logical operator.
  *
  * @param rows       expected row count (finite, non-negative)
  * @param confidence how far the estimate can be trusted, in `(0, 1]`
  * @param source     what the estimate rests on
  */
final case class Estimate(rows: Double, confidence: Double, source: EstimateSource) derives CanEqual:
  require(!rows.isNaN && !rows.isInfinite && rows >= 0, s"invalid row estimate $rows")
  require(confidence > 0 && confidence <= 1, s"invalid confidence $confidence")

/** Construction helpers of [[Estimate]]. */
object Estimate:
  /** Largest row count an estimate reports; keeps every derived figure finite. */
  val MaxRows: Double = 1e15

  /** An estimate with `rows` clamped to `[0, MaxRows]` and `confidence` to `[0.01, 1]`. */
  def bounded(rows: Double, confidence: Double, source: EstimateSource): Estimate =
    val safeRows = if rows.isNaN then MaxRows else rows.max(0).min(MaxRows)
    val safeConfidence = if confidence.isNaN then 0.01 else confidence.max(0.01).min(1.0)
    Estimate(safeRows, safeConfidence, source)
