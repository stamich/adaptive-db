package io.adb.actuator

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the policy and budget defaults and their validation. */
final class PolicyTest:
  @Test def thePolicyDefaultsAreTheOnesOfTheProposal(): Unit =
    val p = Policy()
    assertEquals(50L, p.minExecutions)
    assertEquals(4.0, p.minRatio)
    assertEquals(30L * 60L * 1_000_000_000L, p.cooldownNanos)
    assertEquals(6L * 3600L * 1_000_000_000L, p.rollbackCooldownNanos)
    assertEquals(6, p.verifyWindows)
    assertEquals(2.0, p.minRowsReduction)
    assertEquals(0.25, p.maxWriteSlowdown)
    assertEquals(360, p.idleWindows)

  @Test def theBudgetDefaultsAreTheOnesOfTheProposal(): Unit =
    val b = Budget()
    assertEquals(8, b.maxAdvisorIndexes)
    assertEquals(1L << 30, b.maxIndexBytes)
    assertEquals(1, b.maxConcurrentBuilds)

  @Test def nonsenseValuesAreRefused(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => Policy(minExecutions = 0L))
    assertThrows(classOf[IllegalArgumentException], () => Policy(minRatio = 0.5))
    assertThrows(classOf[IllegalArgumentException], () => Policy(minRowsReduction = 1.0))
    assertThrows(classOf[IllegalArgumentException], () => Policy(verifyWindows = 0))
    assertThrows(classOf[IllegalArgumentException], () => Budget(maxConcurrentBuilds = 0))
    assertThrows(classOf[IllegalArgumentException], () => Budget(bytesPerEntry = 0L))

  @Test def theSizeEstimateScalesWithRows(): Unit =
    val b = Budget(bytesPerEntry = 50L)
    assertEquals(5000L, b.estimateBytes(100L))
    assertEquals(0L, b.estimateBytes(-1L))
