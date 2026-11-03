package io.adb.advisor

/** The heuristic cost-benefit estimate of one candidate action.
  *
  * Both figures are in nanoseconds of work so a benefit can be compared with a cost directly. They
  * are estimates built from sampled telemetry, not measurements; a ratio only ranks candidates and
  * gates them against [[CostBenefitModel.minRatio]].
  *
  * @param benefitNanos expected time saved over the model's horizon
  * @param costNanos    expected time spent to build and maintain the structure over the same horizon
  */
final case class Score(benefitNanos: Double, costNanos: Double) derives CanEqual:
  /** Benefit per unit of cost; infinite for a free action with a benefit, 0 without a benefit. */
  def ratio: Double =
    if benefitNanos <= 0.0 then 0.0
    else if costNanos <= 0.0 then Double.PositiveInfinity
    else benefitNanos / costNanos

/** The numbers behind the advisor's scoring. Every field has a default tuned on the 2.5.2
  * benchmarks; they are heuristics and are meant to be changed per deployment.
  *
  * The benefit of an index is the part of a shape's time spent reading rows it did not need:
  * `(rows read - rows returned) / rows read` of its mean time, times its executions, scaled from
  * the observed span to `horizonNanos`. The cost is one scan of the table to build the index plus
  * a maintenance share proportional to the build (`writePenalty`), because every later write has
  * to update the index too.
  *
  * @param horizonNanos   how far ahead the benefit is projected (default one hour)
  * @param buildNanosPerRow time to put one row into a new index (default 2 µs)
  * @param writePenalty   maintenance cost as a fraction of the build cost over the horizon (default 0.5)
  * @param minRatio       smallest benefit/cost ratio a candidate needs to be recommended (default 2.0)
  * @param minExecutions  fewest captured executions a shape needs before its evidence counts (default 20)
  */
final case class CostBenefitModel(
    horizonNanos: Long = 3_600_000_000_000L,
    buildNanosPerRow: Double = 2_000.0,
    writePenalty: Double = 0.5,
    minRatio: Double = 2.0,
    minExecutions: Long = 20L
) derives CanEqual:
  require(horizonNanos > 0L, "horizonNanos must be positive")
  require(buildNanosPerRow >= 0.0 && writePenalty >= 0.0 && minRatio > 0.0, "model parameters must not be negative")

  /** Expected time an index would save `delta`'s shape over the horizon.
    *
    * @param delta        what the shape did in the observed span
    * @param spanNanos    the observed span (the wall time the delta was collected in)
    * @return nanoseconds saved over [[horizonNanos]], 0 if the evidence is too thin or nothing was wasted
    */
  def indexBenefit(delta: ShapeDelta, spanNanos: Long): Double =
    if delta.executions < minExecutions || spanNanos <= 0L || delta.totalRowsRead <= 0L then 0.0
    else
      val wasted = math.max(delta.totalRowsRead - delta.totalActualRows, 0L).toDouble / delta.totalRowsRead.toDouble
      val savedInSpan = delta.totalNanos.toDouble * wasted
      savedInSpan * (horizonNanos.toDouble / spanNanos.toDouble)

  /** Expected time spent on an index over a table of `rowCount` rows: building it, plus keeping it
    * up to date through the horizon.
    */
  def indexCost(rowCount: Long): Double =
    val build = math.max(rowCount, 1L).toDouble * buildNanosPerRow
    build * (1.0 + writePenalty)

  /** Scores an index candidate. */
  def scoreIndex(benefitNanos: Double, rowCount: Long): Score = Score(benefitNanos, indexCost(rowCount))

  /** Whether a score clears the bar for a recommendation. */
  def worthIt(score: Score): Boolean = score.ratio >= minRatio
