package io.adb.telemetry

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The queue, the sampler wiring and the aggregation. */
final class TelemetryRuntimeTest:
  /** A runtime without a worker thread, so tests drain explicitly. */
  private def runtime(capacity: Int = 8, policy: TelemetrySamplingPolicy = TelemetrySamplingPolicy(fastSampleEvery = 1)) =
    new TelemetryRuntime(capacity, policy, maxShapes = 4, startWorker = false)

  /** An event of shape `id` taking `nanos`. */
  private def event(id: Long, nanos: Long = 100, est: Long = 10, actual: Long = 10) =
    QueryTelemetry(id, 1, est, actual, 50, nanos, cacheHit = true, failed = false)

  /** Off by default; the modes switch it. */
  @Test def offUntilSwitchedOn(): Unit =
    val r = runtime()
    assertFalse(r.enabled)
    r.mode = TelemetryMode.Sampled
    assertTrue(r.enabled)
    r.mode = TelemetryMode.Off
    assertFalse(r.enabled)

  /** A full queue drops events, counts them and never blocks. */
  @Test def fullQueueDropsWithoutBlocking(): Unit =
    val r = runtime(capacity = 2)
    assertTrue(r.offer(event(1)) && r.offer(event(1)))
    assertFalse(r.offer(event(1)))
    val m = r.metrics
    assertEquals((2L, 1L, 2), (m.accepted, m.dropped, m.queueSize))
    assertEquals(2, r.drainNow())
    assertTrue(r.offer(event(1)))

  /** Sampling counts skips and captures; Full captures everything. */
  @Test def samplerCounts(): Unit =
    val r = runtime(policy = TelemetrySamplingPolicy(fastSampleEvery = 4, alwaysUnknown = false))
    r.mode = TelemetryMode.Sampled
    val captured = (1 to 400).count(_ => r.decide(7, 10, failed = false) == SamplingDecision.Capture)
    assertEquals(100, captured)
    assertEquals((300L, 100L), (r.metrics.skipped, r.metrics.captured))
    r.mode = TelemetryMode.Full
    assertEquals(SamplingDecision.Capture, r.decide(7, 10, failed = false))

  /** An unknown shape is captured until it is registered. */
  @Test def unknownShapesAreCaptured(): Unit =
    val r = runtime(policy = TelemetrySamplingPolicy(fastSampleEvery = 1024))
    r.mode = TelemetryMode.Sampled
    val shape = QueryShape(Vector(1), Vector.empty, Vector.empty, Vector.empty, false, false)
    // Sequence number 1 is not a multiple of 1024, so only the "unknown shape" rule can capture.
    assertEquals(SamplingDecision.Capture, r.decide(shape.id, 1, failed = false))
    r.registry.register(shape)
    assertEquals(SamplingDecision.Skip, r.decide(shape.id, 1, failed = false))

  /** Drained events are aggregated per shape, bounded by maxShapes. */
  @Test def aggregation(): Unit =
    val r = runtime(capacity = 64)
    r.offer(event(1, nanos = 100, est = 10, actual = 400))
    r.offer(event(1, nanos = 300))
    r.offer(event(2, nanos = 50))
    assertEquals(3, r.drainNow())
    val s = r.stats(1).get
    assertEquals((2L, 2L, 300L), (s.executions, s.cacheHits, s.maxNanos))
    assertEquals(200.0, s.meanNanos)
    assertEquals(40.0, s.maxQError)
    assertEquals(Vector(1L, 2L), r.allStats().map(_.shapeId))
    for id <- 3L to 9L do r.offer(event(id))
    r.drainNow()
    assertTrue(r.allStats().size <= 4)

  /** The background worker folds events on its own and stops when closed. */
  @Test def workerDrainsInTheBackground(): Unit =
    val r = new TelemetryRuntime(16, TelemetrySamplingPolicy(), 4, startWorker = true)
    try
      r.offer(event(5)); r.offer(event(5))
      val deadline = System.nanoTime() + 5_000_000_000L
      while r.stats(5).forall(_.executions < 2) && System.nanoTime() < deadline do Thread.sleep(5)
      assertEquals(2L, r.stats(5).get.executions)
      assertEquals(2L, r.metrics.drained)
    finally r.close()
