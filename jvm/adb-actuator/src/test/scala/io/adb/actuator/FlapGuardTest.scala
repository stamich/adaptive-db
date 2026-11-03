package io.adb.actuator

import io.adb.advisor.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the flap guard: a target rolled back repeatedly waits longer each time. */
final class FlapGuardTest:
  private val second = 1_000_000_000L
  private val hour = 3600L * second
  private val policy = Policy()

  @Test def theWaitDoublesWithEachRollbackUpToEightTimes(): Unit =
    val base = policy.rollbackCooldownNanos
    assertEquals(0L, FlapGuard.rollbackCooldown(policy, 0))
    assertEquals(base, FlapGuard.rollbackCooldown(policy, 1))
    assertEquals(2 * base, FlapGuard.rollbackCooldown(policy, 2))
    assertEquals(4 * base, FlapGuard.rollbackCooldown(policy, 3))
    assertEquals(8 * base, FlapGuard.rollbackCooldown(policy, 4))
    assertEquals(8 * base, FlapGuard.rollbackCooldown(policy, 40), "capped")

  private val recommendation = CreateIndex(7L, "orders", Vector(2L), Vector("customer_id"), Some(Score(500.0, 100.0)), Vector(11L))
  private val design = PhysicalDesign.of(Seq(EntityDesign(7L, "orders", Map(2L -> "customer_id"), 1000L, None, Vector.empty)))
  private val workload = WorkloadSnapshot(Vector.empty, Map(11L -> ShapeDelta(11L, 500L, 0L, 5L, 5L, 500_000L, 1.0)))

  /** Gate decision at `sinceRollbackHours` after the latest of `rollbacks` rollbacks. */
  private def decideAfter(rollbacks: Int, sinceRollbackHours: Long): GateDecision =
    val now = 1000L * hour
    PolicyGate.decide(recommendation, design, workload, policy, Budget(),
      GateContext(now, lastActionNanos = Some(now - sinceRollbackHours * hour), lastRollbackNanos = Some(now - sinceRollbackHours * hour), rollbacks = rollbacks))

  @Test def theGateHoldsASecondRollbackForTwelveHours(): Unit =
    assertEquals(Approve, decideAfter(rollbacks = 1, sinceRollbackHours = 7))
    assertTrue(decideAfter(rollbacks = 2, sinceRollbackHours = 7).isInstanceOf[Reject], "7 h is not enough after two rollbacks")
    assertEquals(Approve, decideAfter(rollbacks = 2, sinceRollbackHours = 13))

  @Test def aFlappingWorkloadCreatesTheIndexFewTimesInADay(): Unit =
    // Script: every time the gate lets the advisor create the index it is rolled back at once. The
    // recommendation is offered every 10 minutes for 48 hours.
    var rollbacks = 0
    var lastRollback = Option.empty[Long]
    var creates = 0
    var t = 0L
    val step = 600L * second
    while t < 48L * hour do
      val context = GateContext(t, lastActionNanos = lastRollback, lastRollbackNanos = lastRollback, rollbacks = rollbacks)
      if PolicyGate.decide(recommendation, design, workload, policy, Budget(), context) == Approve then
        creates += 1; rollbacks += 1; lastRollback = Some(t)
      t += step
    assertTrue(creates <= 4, s"created $creates times in 48 h; without the guard it would be 8 (every 6 h)")
    assertTrue(creates >= 3)
