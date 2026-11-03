package io.adb.actuator

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the build guard. */
final class BuildGuardTest:
  @Test def atMostTheLimitRunsAtOnce(): Unit =
    val g = new BuildGuard(1)
    assertTrue(g.tryBegin())
    assertFalse(g.tryBegin(), "the second build waits")
    assertEquals(1, g.inFlight)
    g.end()
    assertTrue(g.tryBegin())

  @Test def aHigherLimitAllowsMore(): Unit =
    val g = new BuildGuard(2)
    assertTrue(g.tryBegin()); assertTrue(g.tryBegin()); assertFalse(g.tryBegin())

  @Test def guardedReleasesTheSlotEvenWhenTheBodyFails(): Unit =
    val g = new BuildGuard(1)
    assertThrows(classOf[IllegalStateException], () => g.guarded[Int](throw new IllegalStateException("boom")))
    assertEquals(0, g.inFlight)
    assertEquals(Some(5), g.guarded(5))

  @Test def guardedDoesNotRunTheBodyWhenFull(): Unit =
    val g = new BuildGuard(1)
    g.tryBegin()
    var ran = false
    assertEquals(None, g.guarded { ran = true; 1 })
    assertFalse(ran)

  @Test def afterShutdownNothingNewStartsButARunningBuildIsNotInterrupted(): Unit =
    val g = new BuildGuard(2)
    assertTrue(g.tryBegin())
    g.shutdown()
    assertTrue(g.isShutdown)
    assertFalse(g.tryBegin())
    assertEquals(1, g.inFlight, "the build that began before shutdown is still counted")
    g.end()
    assertEquals(0, g.inFlight)

  @Test def endNeverGoesBelowZero(): Unit =
    val g = new BuildGuard(1)
    g.end(); g.end()
    assertEquals(0, g.inFlight)
    assertTrue(g.tryBegin())
    assertFalse(g.tryBegin())

  @Test def aNonPositiveLimitIsRefused(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => new BuildGuard(0))
