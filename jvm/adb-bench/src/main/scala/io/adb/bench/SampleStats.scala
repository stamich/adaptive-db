package io.adb.bench

/** Whether the batch medians of a case agree closely enough to trust it. */
enum Stability(val label: String):
  /** The batch MAD is within the limit. */
  case Stable extends Stability("stable")

  /** The batch MAD is beyond the limit: remeasure or call the case unstable. */
  case Unstable extends Stability("unstable")

  /** A quick plan does not judge. */
  case Skipped extends Stability("skipped")

/** Timing samples of one case and their summaries; all times are nanoseconds per invocation.
  *
  * The headline is `medianNs`, the **median of the batch medians**. `batchMadPercent` (the median
  * absolute deviation of the batch medians, in percent of the headline) is the stability input.
  * Outliers (beyond 3 x 1.4826 x MAD of the samples) are reported, never removed.
  *
  * @param samplesNs           every sample, in measurement order
  * @param batchMediansNs      the median of each batch
  * @param samplesPerBatch     samples in the first batch
  * @param iterationsPerSample invocations folded into one sample by the calibration
  * @param medianNs            median of the batch medians (the headline)
  * @param globalMedianNs      median of all samples
  * @param minNs               fastest sample
  * @param maxNs               slowest sample
  * @param p95Ns               95th percentile (nearest rank)
  * @param p99Ns               99th percentile (nearest rank)
  * @param meanNs              mean of the samples
  * @param stddevNs            population standard deviation of the samples
  * @param cvPercent           standard deviation in percent of the mean
  * @param madNs               median absolute deviation of the samples
  * @param batchMadPercent     median absolute deviation of the batch medians, percent of the headline
  * @param outlierIndices      indices of the outlying samples
  * @param quick               whether a quick plan produced the samples
  */
final case class SampleStats(
    samplesNs: Vector[Double],
    batchMediansNs: Vector[Double],
    samplesPerBatch: Int,
    iterationsPerSample: Long,
    medianNs: Double,
    globalMedianNs: Double,
    minNs: Double,
    maxNs: Double,
    p95Ns: Double,
    p99Ns: Double,
    meanNs: Double,
    stddevNs: Double,
    cvPercent: Double,
    madNs: Double,
    batchMadPercent: Double,
    outlierIndices: Vector[Int],
    quick: Boolean
):
  /** The stability verdict for a batch-MAD limit; a quick plan gives [[Stability.Skipped]]. */
  def stability(limitPercent: Double): Stability =
    if quick then Stability.Skipped
    else if batchMadPercent <= limitPercent then Stability.Stable
    else Stability.Unstable

/** Constructors of [[SampleStats]] and the statistics they use. */
object SampleStats:
  /** Batch-MAD limit (percent) when a family sets none; a starting value, to be replaced by limits
    * measured on the reference machine (see `docs/BENCHMARKS.md`).
    */
  val DefaultStableBatchMadPercent: Double = 5.0

  /** Ratio between the MAD and the standard deviation of a normal distribution. */
  val MadSigmaScale: Double = 1.4826

  /** Scaled MADs beyond which a sample is an outlier. */
  val OutlierSigmas: Double = 3.0

  /** Summarises batches of samples (nanoseconds per invocation).
    *
    * @throws IllegalArgumentException if there is no sample
    */
  def fromBatches(batches: Seq[Seq[Double]], iterationsPerSample: Long, quick: Boolean): SampleStats =
    val samples = batches.flatten.toVector
    require(samples.nonEmpty, "a case needs at least one sample")
    val batchMedians = batches.map(b => median(b)).toVector
    val med = median(batchMedians)
    val globalMed = median(samples)
    val mad = median(samples.map(s => math.abs(s - globalMed)))
    val batchMad = median(batchMedians.map(m => math.abs(m - med)))
    val batchMadPercent = if med > 0.0 then batchMad / med * 100.0 else 0.0
    val n = samples.size.toDouble
    val mean = samples.sum / n
    val stddev = math.sqrt(samples.map(s => (s - mean) * (s - mean)).sum / n)
    val sorted = samples.sorted
    val threshold = OutlierSigmas * MadSigmaScale * mad
    val outliers =
      if mad > 0.0 then samples.indices.filter(i => math.abs(samples(i) - globalMed) > threshold).toVector
      else Vector.empty[Int]
    SampleStats(
      samples, batchMedians, batches.headOption.fold(0)(_.size), iterationsPerSample, med, globalMed,
      sorted.head, sorted.last, nearestRank(sorted, 0.95), nearestRank(sorted, 0.99), mean, stddev,
      if mean > 0.0 then stddev / mean * 100.0 else 0.0, mad, batchMadPercent, outliers, quick
    )

  /** Summarises the output of [[Runner.measure]]. */
  def fromMeasured[T](measured: Measured[T]): SampleStats =
    fromBatches(measured.batchesNs, measured.iterationsPerSample, measured.plan.isQuick)

  /** Median of values (the mean of the two middle values for an even count); `0.0` for none. */
  def median(values: Seq[Double]): Double =
    if values.isEmpty then 0.0
    else
      val sorted = values.sorted
      val mid = sorted.size / 2
      if sorted.size % 2 == 1 then sorted(mid) else (sorted(mid - 1) + sorted(mid)) / 2.0

  /** Nearest-rank percentile of an ascending sequence: the smallest value that is at least `p`. */
  def nearestRank(sorted: Vector[Double], p: Double): Double =
    val rank = math.ceil(p * sorted.size).toInt
    sorted(math.max(1, math.min(rank, sorted.size)) - 1)
