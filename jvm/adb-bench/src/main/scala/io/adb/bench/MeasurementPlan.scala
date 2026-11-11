package io.adb.bench

/** How a case is measured: warm-up, calibration, batches and samples per batch.
  *
  * The same shape as the Rust `MeasurementPlan` of `adb-bench`, so a JVM case and a Rust case of one
  * report are measured alike. Durations are in nanoseconds.
  *
  * @param warmupInvocations invocations run and discarded before anything is timed (JIT, caches)
  * @param minSampleNs       calibration doubles the iterations of one sample until the group lasts
  *                          at least this long (zero: one invocation per sample)
  * @param maxIterations     upper bound on the iterations of one sample
  * @param batches           number of batches of samples
  * @param samplesPerBatch   samples per batch
  */
final case class MeasurementPlan(
    warmupInvocations: Int,
    minSampleNs: Long,
    maxIterations: Long,
    batches: Int,
    samplesPerBatch: Int
):
  /** Whether this is a quick (smoke) plan, which can never produce a release verdict. */
  def isQuick: Boolean = batches < 3

  /** Samples taken in total. */
  def totalSamples: Int = batches * samplesPerBatch

  /** The plan as JSON (durations in milliseconds). */
  def json: String = Json.obj(
    "warmup_invocations" -> Json.num(warmupInvocations.toLong),
    "min_sample_ms" -> Json.num(minSampleNs / 1e6),
    "max_iterations" -> Json.num(maxIterations),
    "batches" -> Json.num(batches.toLong),
    "samples_per_batch" -> Json.num(samplesPerBatch.toLong),
    "quick" -> Json.bool(isQuick)
  )

/** The standard plans and the environment switches that choose among them. */
object MeasurementPlan:
  /** Environment variable that selects the quick plans (`1`). */
  val QuickEnv = "ADB_BENCH_QUICK"

  /** Environment variable that overrides the minimum sample duration of CPU-bound plans, in ms. */
  val MinSampleMsEnv = "ADB_BENCH_MIN_SAMPLE_MS"

  /** CPU-bound release plan: 5 warm-ups, samples of at least 50 ms, 3 batches of 7. Up to 2^26
    * iterations per sample, so a case of a few nanoseconds still fills 50 ms (with 2^20 a 2 ns case
    * gave 2 ms samples, and a few ms of background compilation made it look cold).
    */
  val Release: MeasurementPlan = MeasurementPlan(5, 50_000_000L, 1L << 26, 3, 7)

  /** A smoke plan: one batch of 5 samples of at least 2 ms. */
  val Quick: MeasurementPlan = MeasurementPlan(1, 2_000_000L, 1L << 20, 1, 5)

  /** Plan for fsync-bound operations: one invocation per sample, 5 batches of 5. */
  val Durable: MeasurementPlan = MeasurementPlan(2, 0L, 1L, 5, 5)

  /** The durable plan in its quick form: one batch of 3 samples. */
  val DurableQuick: MeasurementPlan = MeasurementPlan(1, 0L, 1L, 1, 3)

  /** The plan for `durable` or CPU-bound cases, from the process environment. */
  def fromEnv(durable: Boolean): MeasurementPlan = fromLookup(durable, name => Option(System.getenv(name)))

  /** Like [[fromEnv]] with an injectable variable lookup, so tests need not touch the environment. */
  def fromLookup(durable: Boolean, lookup: String => Option[String]): MeasurementPlan =
    val quick = lookup(QuickEnv).contains("1")
    val plan = (durable, quick) match
      case (false, false) => Release
      case (false, true) => Quick
      case (true, false) => Durable
      case (true, true) => DurableQuick
    if durable then plan
    else lookup(MinSampleMsEnv).flatMap(_.toLongOption).fold(plan)(ms => plan.copy(minSampleNs = ms * 1_000_000L))
