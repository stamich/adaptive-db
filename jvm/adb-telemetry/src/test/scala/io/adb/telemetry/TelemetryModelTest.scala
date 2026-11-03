package io.adb.telemetry

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Shapes, sampling decisions, events and the shape registry. */
final class TelemetryModelTest:
  /** A shape over entities 1 and 2. */
  private def shape(limit: Boolean = true) =
    QueryShape(Vector(1, 2), Vector(FieldRef(2, 5)), Vector(JoinEdge(1, 0, 2, 3)), Vector(OrderRef(FieldRef(2, 6), true)), hasAggregate = false, hasLimit = limit)

  /** Equal shapes share an id; any difference changes it. */
  @Test def shapeIdFollowsContent(): Unit =
    assertEquals(shape().id, shape().id)
    assertNotEquals(shape().id, shape(limit = false).id)
    assertNotEquals(shape().id, shape().copy(entities = Vector(1, 3)).id)
    assertNotEquals(shape().id, shape().copy(orderFields = Vector(OrderRef(FieldRef(2, 6), false))).id)

  /** Failures, slow and unknown queries are always captured; known fast ones are sampled 1 in N. */
  @Test def samplingRules(): Unit =
    val p = TelemetrySamplingPolicy(fastSampleEvery = 4, slowQueryNanos = 1000)
    assertEquals(SamplingDecision.Capture, p.decide(1, 10, failed = true, knownShape = true))
    assertEquals(SamplingDecision.Capture, p.decide(1, 1000, failed = false, knownShape = true))
    assertEquals(SamplingDecision.Capture, p.decide(1, 10, failed = false, knownShape = false))
    val captured = (0L until 400L).count(i => p.decide(i, 10, failed = false, knownShape = true) == SamplingDecision.Capture)
    assertEquals(100, captured)
    val noUnknown = p.copy(alwaysUnknown = false)
    assertEquals(SamplingDecision.Skip, noUnknown.decide(1, 10, failed = false, knownShape = false))

  /** The rate must be a power of two. */
  @Test def policyIsValidated(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => TelemetrySamplingPolicy(fastSampleEvery = 3))
    assertThrows(classOf[IllegalArgumentException], () => TelemetrySamplingPolicy(fastSampleEvery = 0))
    assertEquals(SamplingDecision.Capture, TelemetrySamplingPolicy(fastSampleEvery = 1).decide(7, 1, false, true))

  /** The q-error is symmetric and 1 for an exact or missing estimate. */
  @Test def qError(): Unit =
    def event(est: Long, act: Long) = QueryTelemetry(1, 1, est, act, -1, 1, false, false)
    assertEquals(1.0, event(10, 10).qError)
    assertEquals(40.0, event(10, 400).qError)
    assertEquals(40.0, event(400, 10).qError)
    assertEquals(10.0, event(10, 0).qError)
    assertEquals(1.0, event(-1, 5000).qError)

  /** The registry is idempotent and bounded. */
  @Test def registryIsBounded(): Unit =
    val r = new QueryShapeRegistry(2)
    val a = shape(); val b = shape(limit = false); val c = shape().copy(entities = Vector(9))
    assertTrue(r.register(a) && r.register(a) && r.register(b))
    assertFalse(r.register(c))
    assertEquals(2, r.size)
    assertTrue(r.contains(a.id) && !r.contains(c.id))
    assertEquals(Some(b), r.get(b.id))
    r.clear()
    assertEquals(0, r.size)
