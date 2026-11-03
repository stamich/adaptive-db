package io.adb.telemetry

/** Counters of a [[TelemetryRuntime]] at one moment.
  *
  * `skipped + captured` is the number of queries that reached the sampler; `captured` equals
  * `accepted + dropped`.
  *
  * @param mode               current mode
  * @param skipped            queries the sampler skipped
  * @param captured           queries the sampler chose to capture
  * @param accepted           events placed in the queue
  * @param dropped            events lost because the queue was full
  * @param drained            events the worker has folded into the profiles
  * @param queueHighWatermark largest queue length seen when the worker drained
  * @param queueSize          events waiting now
  * @param shapes             shapes registered
  */
final case class TelemetryMetrics(
    mode: TelemetryMode,
    skipped: Long,
    captured: Long,
    accepted: Long,
    dropped: Long,
    drained: Long,
    queueHighWatermark: Int,
    queueSize: Int,
    shapes: Int
)

/** What the worker has learned about one shape.
  *
  * @param shapeId        the shape
  * @param executions     captured executions
  * @param cacheHits      of which used a cached plan
  * @param failures       of which failed
  * @param totalNanos     sum of elapsed time
  * @param maxNanos       slowest execution
  * @param totalActualRows sum of rows returned
  * @param totalRowsRead  sum of rows scanned, over executions that reported it
  * @param maxQError      worst estimate error seen
  */
final case class ShapeStats(
    shapeId: Long,
    executions: Long,
    cacheHits: Long,
    failures: Long,
    totalNanos: Long,
    maxNanos: Long,
    totalActualRows: Long,
    totalRowsRead: Long,
    maxQError: Double
):
  /** Mean elapsed nanoseconds per captured execution. */
  def meanNanos: Double = if executions == 0 then 0.0 else totalNanos.toDouble / executions
