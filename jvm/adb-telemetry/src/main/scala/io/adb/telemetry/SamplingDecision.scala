package io.adb.telemetry

/** What to do with the telemetry of one finished query. An enum of singletons, so deciding
  * allocates nothing.
  */
enum SamplingDecision derives CanEqual:
  /** Do nothing: no shape lookup, no event, no profile parsing. */
  case Skip
  /** Build the compact event and offer it to the queue. */
  case Capture

/** How the telemetry sampler behaves.
  *
  * Telemetry is a mode, not just a rate: `Off` makes the whole path a single branch.
  */
enum TelemetryMode derives CanEqual:
  /** No telemetry at all. */
  case Off
  /** Sample according to the [[TelemetrySamplingPolicy]]. */
  case Sampled
  /** Capture every query (tests, demos, short diagnostic runs). */
  case Full

/** The sampling rules, evaluated from values the gateway already has when a query finishes.
  *
  * A query is captured when it failed, ran at least `slowQueryNanos`, has a shape the workload
  * model has not seen, or is picked by the deterministic 1-in-`fastSampleEvery` counter. All other
  * queries are skipped using one branch chain and one counter increment (no randomness, no
  * allocation).
  *
  * @param fastSampleEvery  capture one fast, known query in this many; a power of two (1 = all)
  * @param slowQueryNanos   queries at least this slow are always captured
  * @param alwaysFailures   capture every failed query
  * @param alwaysUnknown    capture a query whose shape has not been registered yet
  */
final case class TelemetrySamplingPolicy(
    fastSampleEvery: Int = 128,
    slowQueryNanos: Long = 10_000_000L,
    alwaysFailures: Boolean = true,
    alwaysUnknown: Boolean = true
):
  require(fastSampleEvery >= 1 && (fastSampleEvery & (fastSampleEvery - 1)) == 0, "fastSampleEvery must be a power of two")
  require(slowQueryNanos >= 0, "slowQueryNanos must not be negative")

  /** Mask applied to the counter: capture when `counter & mask == 0`. */
  private val mask: Long = (fastSampleEvery - 1).toLong

  /** Decides from cheap facts only.
    *
    * @param counter      a running count of finished queries (any monotonically increasing value)
    * @param elapsedNanos wall-clock time of the query
    * @param failed       whether the query failed
    * @param knownShape   whether the query's shape is already registered
    */
  def decide(counter: Long, elapsedNanos: Long, failed: Boolean, knownShape: Boolean): SamplingDecision =
    if failed && alwaysFailures then SamplingDecision.Capture
    else if elapsedNanos >= slowQueryNanos then SamplingDecision.Capture
    else if !knownShape && alwaysUnknown then SamplingDecision.Capture
    else if (counter & mask) == 0L then SamplingDecision.Capture
    else SamplingDecision.Skip
