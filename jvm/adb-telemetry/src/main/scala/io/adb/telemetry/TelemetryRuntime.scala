package io.adb.telemetry

import java.util.ArrayList
import java.util.concurrent.{ArrayBlockingQueue, ConcurrentHashMap, TimeUnit}
import java.util.concurrent.atomic.{AtomicLong, LongAdder}

/** The telemetry pipeline of one database: sampler, bounded queue, shape registry and a worker
  * that folds events into per-shape statistics.
  *
  * **Hot path.** A finished query calls [[enabled]] (one volatile read). If telemetry is on it
  * calls [[decide]] with values it already has; only for [[SamplingDecision.Capture]] does it
  * build a [[QueryTelemetry]] and call [[offer]]. `offer` uses `ArrayBlockingQueue.offer`, which
  * never blocks: a full queue drops the event and counts it.
  *
  * **Off the hot path.** Everything else (draining, aggregation) runs in a daemon worker thread,
  * or on demand through [[drainNow]] (tests and demos use that to stay deterministic).
  *
  * @param capacity    queue length; events beyond it are dropped
  * @param policy      sampling rules for [[TelemetryMode.Sampled]]
  * @param maxShapes   bound of the shape registry and of the per-shape statistics
  * @param startWorker whether the background worker may run (it starts when telemetry is first
  *                    switched on, so a database that never uses telemetry has no extra thread)
  */
final class TelemetryRuntime(
    capacity: Int = 4096,
    @volatile var policy: TelemetrySamplingPolicy = TelemetrySamplingPolicy(),
    maxShapes: Int = 4096,
    startWorker: Boolean = true
) extends TelemetrySink with AutoCloseable:
  require(capacity > 0, "capacity must be positive")

  /** Current mode; written rarely, read on every query. */
  @volatile private var currentMode: TelemetryMode = TelemetryMode.Off
  /** The registered shapes. */
  val registry: QueryShapeRegistry = new QueryShapeRegistry(maxShapes)
  /** Pending events. */
  private val queue = new ArrayBlockingQueue[QueryTelemetry](capacity)
  /** Running count of finished queries seen by the sampler (drives the 1-in-N choice). */
  private val sequence = new AtomicLong()
  private val skippedCount, capturedCount, acceptedCount, droppedCount, drainedCount = new LongAdder()
  /** Largest queue length seen by the worker. */
  @volatile private var highWatermark: Int = 0
  /** Folded statistics by shape id. */
  private val profiles = new ConcurrentHashMap[Long, Accumulator]()
  /** Set by [[close]] to stop the worker. */
  @volatile private var closed = false

  /** The background worker; started the first time telemetry is switched on. */
  @volatile private var worker: Thread = null

  /** Starts the worker once (if allowed). */
  private def ensureWorker(): Unit =
    if startWorker && worker == null then
      synchronized {
        if worker == null && !closed then
          val t = new Thread(() => runWorker(), "adb-telemetry")
          t.setDaemon(true)
          t.start()
          worker = t
      }

  /** The current mode. */
  def mode: TelemetryMode = currentMode

  /** Changes the mode; takes effect for queries that finish afterwards. */
  def mode_=(next: TelemetryMode): Unit =
    currentMode = next
    if next != TelemetryMode.Off then ensureWorker()

  /** True unless the mode is [[TelemetryMode.Off]]. */
  def enabled: Boolean = currentMode != TelemetryMode.Off

  /** The sampling decision for a finished query (counts it as skipped or captured).
    *
    * @param shapeId      id of the query's shape
    * @param elapsedNanos wall-clock time of the statement
    * @param failed       whether it failed
    */
  def decide(shapeId: Long, elapsedNanos: Long, failed: Boolean): SamplingDecision =
    val decision =
      if currentMode == TelemetryMode.Full then SamplingDecision.Capture
      else policy.decide(sequence.incrementAndGet(), elapsedNanos, failed, registry.contains(shapeId))
    if decision == SamplingDecision.Capture then capturedCount.increment() else skippedCount.increment()
    decision

  /** Offers an event without blocking; false if the queue was full (the event is dropped). */
  def offer(event: QueryTelemetry): Boolean =
    if queue.offer(event) then
      acceptedCount.increment()
      true
    else
      droppedCount.increment()
      false

  /** Folds up to `max` waiting events into the statistics; returns how many were folded. */
  def drainNow(max: Int = Int.MaxValue): Int =
    val size = queue.size
    if size > highWatermark then highWatermark = size
    val batch = new ArrayList[QueryTelemetry](math.min(size, 1024))
    val n = queue.drainTo(batch, max)
    var i = 0
    while i < n do
      fold(batch.get(i))
      i += 1
    drainedCount.add(n)
    n

  /** The statistics of one shape. */
  def stats(shapeId: Long): Option[ShapeStats] = Option(profiles.get(shapeId)).map(_.snapshot(shapeId))

  /** The statistics of every shape, busiest first. */
  def allStats(): Vector[ShapeStats] =
    val out = Vector.newBuilder[ShapeStats]
    profiles.forEach((id, acc) => out += acc.snapshot(id))
    out.result().sortBy(s => -s.totalNanos)

  /** Snapshot of the counters. */
  def metrics: TelemetryMetrics = TelemetryMetrics(
    currentMode, skippedCount.sum, capturedCount.sum, acceptedCount.sum, droppedCount.sum,
    drainedCount.sum, highWatermark, queue.size, registry.size
  )

  /** Stops the worker (events still queued are discarded). */
  def close(): Unit =
    closed = true
    val w = worker
    if w != null then w.interrupt()

  /** Adds one event to its shape's accumulator, unless the per-shape bound is reached. */
  private def fold(event: QueryTelemetry): Unit =
    var acc = profiles.get(event.shapeId)
    if acc == null && profiles.size < maxShapes then
      acc = new Accumulator
      val prior = profiles.putIfAbsent(event.shapeId, acc)
      if prior != null then acc = prior
    if acc != null then acc.add(event)

  /** The worker loop: wait for events, fold them, repeat until closed. */
  private def runWorker(): Unit =
    try
      while !closed do
        val first = queue.poll(200, TimeUnit.MILLISECONDS)
        if first != null then
          fold(first)
          drainedCount.increment()
          drainNow(1024)
    catch case _: InterruptedException => ()

  /** Mutable per-shape totals. Only the worker (or one `drainNow` caller at a time) writes. */
  private final class Accumulator:
    private var executions, cacheHits, failures, totalNanos, maxNanos, totalActual, totalRead = 0L
    private var maxQ = 1.0

    /** Folds `e` in. */
    def add(e: QueryTelemetry): Unit = synchronized {
      executions += 1
      if e.cacheHit then cacheHits += 1
      if e.failed then failures += 1
      totalNanos += e.elapsedNanos
      if e.elapsedNanos > maxNanos then maxNanos = e.elapsedNanos
      totalActual += e.actualRows
      if e.rowsRead > 0 then totalRead += e.rowsRead
      val q = e.qError
      if q > maxQ then maxQ = q
    }

    /** An immutable copy. */
    def snapshot(id: Long): ShapeStats = synchronized {
      ShapeStats(id, executions, cacheHits, failures, totalNanos, maxNanos, totalActual, totalRead, maxQ)
    }
