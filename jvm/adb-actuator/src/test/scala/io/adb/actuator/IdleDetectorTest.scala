package io.adb.actuator

import io.adb.advisor.{ShapeDelta, WorkloadWindow}
import io.adb.telemetry.{FieldRef, JoinEdge, QueryShape}
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of idle-index detection. */
final class IdleDetectorTest:
  private val index = OwnedIndex("adv_orders_customer_id", 7L, "orders", Vector(2L), ActionTarget(7L, "index:7:2"), Some(ActionId(1)), 0L, 1L)
  private val user = QueryShape(Vector(7L), Vector(FieldRef(7L, 2L)), Vector.empty, Vector.empty, false, false)
  private val other = QueryShape(Vector(7L), Vector(FieldRef(7L, 3L)), Vector.empty, Vector.empty, false, false)
  private val join = QueryShape(Vector(7L, 8L), Vector.empty, Vector(JoinEdge(7L, 2L, 8L, 1L)), Vector.empty, false, false)
  private val shapes = Map(user.id -> user, other.id -> other, join.id -> join)
  private def window(ids: Long*) =
    WorkloadWindow(0L, 10L, ids.map(id => id -> ShapeDelta(id, 5L, 0L, 5L, 5L, 5L, 1.0)).toMap)

  @Test def aWindowThatWantsTheIndexResetsTheCount(): Unit =
    val d = new IdleDetector(3)
    d.observe(Vector(index), window(other.id), shapes.get)
    d.observe(Vector(index), window(other.id), shapes.get)
    assertEquals(2, d.idleCount(index.name))
    d.observe(Vector(index), window(user.id), shapes.get)
    assertEquals(0, d.idleCount(index.name))

  @Test def aJoinThroughTheLeadingColumnCountsAsUse(): Unit =
    val d = new IdleDetector(3)
    d.observe(Vector(index), window(other.id), shapes.get)
    d.observe(Vector(index), window(join.id), shapes.get)
    assertEquals(0, d.idleCount(index.name))

  @Test def itIsReportedAfterIdleWindowsConsecutiveWindowsWithoutUse(): Unit =
    val d = new IdleDetector(3)
    assertTrue(d.observe(Vector(index), window(other.id), shapes.get).isEmpty)
    assertTrue(d.observe(Vector(index), window(other.id), shapes.get).isEmpty)
    assertEquals(Vector(index), d.observe(Vector(index), window(other.id), shapes.get))

  @Test def aQuietWindowNeitherCountsNorResets(): Unit =
    val d = new IdleDetector(3)
    d.observe(Vector(index), window(other.id), shapes.get)
    d.observe(Vector(index), window(), shapes.get)
    d.observe(Vector(index), window(), shapes.get)
    assertEquals(1, d.idleCount(index.name), "nothing ran in the two quiet windows")
    d.observe(Vector(index), window(other.id), shapes.get)
    assertEquals(2, d.idleCount(index.name))

  @Test def onlyHandedOverIndexesAreTrackedSoAUserIndexIsNeverReported(): Unit =
    val d = new IdleDetector(1)
    val reported = d.observe(Vector.empty, window(other.id), shapes.get)
    assertTrue(reported.isEmpty)
    assertEquals(0, d.idleCount("ix_user"))

  @Test def forgetStartsOver(): Unit =
    val d = new IdleDetector(5)
    d.observe(Vector(index), window(other.id), shapes.get)
    d.forget(index.name)
    assertEquals(0, d.idleCount(index.name))

  @Test def aShapeTheLookupDoesNotKnowIsIgnored(): Unit =
    val d = new IdleDetector(1)
    assertEquals(Vector(index), d.observe(Vector(index), window(12345L), shapes.get))
