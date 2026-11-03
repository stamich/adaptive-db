package io.adb.advisor

import java.util.concurrent.atomic.AtomicInteger
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the advisor's timer. */
final class AdvisorSchedulerTest:
  private val advisor = new Advisor(() => Vector.empty, _ => None)

  /** Waits up to five seconds for `condition`. */
  private def eventually(condition: => Boolean): Boolean =
    val deadline = System.nanoTime() + 5_000_000_000L
    while !condition && System.nanoTime() < deadline do Thread.sleep(5)
    condition

  @Test def itEvaluatesRepeatedlyWhileStartedAndStopsOnStop(): Unit =
    val calls = new AtomicInteger()
    val scheduler = new AdvisorScheduler(advisor, () => { calls.incrementAndGet(); PhysicalDesign.empty }, 10L)
    assertFalse(scheduler.running)
    scheduler.start()
    scheduler.start() // a second start changes nothing
    assertTrue(scheduler.running)
    assertTrue(eventually(calls.get >= 3), "expected several evaluations")
    scheduler.stop()
    assertFalse(scheduler.running)
    val after = calls.get
    Thread.sleep(60)
    assertTrue(calls.get <= after + 1, "no evaluations after stop")

  @Test def aFailingEvaluationIsReportedAndDoesNotStopTheTimer(): Unit =
    val errors = new AtomicInteger()
    val scheduler = new AdvisorScheduler(advisor, () => throw new IllegalStateException("boom"), 10L, _ => errors.incrementAndGet())
    scheduler.start()
    assertTrue(eventually(errors.get >= 2), "the timer keeps going after an error")
    scheduler.close()
    assertFalse(scheduler.running)

  @Test def aNonPositivePeriodIsRejected(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => new AdvisorScheduler(advisor, () => PhysicalDesign.empty, 0L))
