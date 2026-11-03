package io.adb.telemetry

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the per-entity write counters. */
final class WriteCountersTest:
  @Test def anEntityNeverWrittenHasZeroStats(): Unit =
    val c = new WriteCounters
    assertEquals(WriteStats(9L, 0L, 0L, 0L), c.stats(9L))
    assertEquals(0.0, c.stats(9L).meanNanos)
    assertTrue(c.all().isEmpty)

  @Test def recordsAccumulatePerEntity(): Unit =
    val c = new WriteCounters
    c.record(1L, 1L, 100L); c.record(1L, 3L, 300L); c.record(2L, 1L, 50L)
    assertEquals(WriteStats(1L, 2L, 4L, 400L), c.stats(1L))
    assertEquals(200.0, c.stats(1L).meanNanos)
    assertEquals(Vector(1L, 2L), c.all().map(_.entityId))

  @Test def sinceSubtractsAnEarlierSnapshot(): Unit =
    val c = new WriteCounters
    c.record(1L, 1L, 100L)
    val before = c.stats(1L)
    c.record(1L, 2L, 500L)
    assertEquals(WriteStats(1L, 1L, 2L, 500L), c.stats(1L).since(before))

  @Test def sinceReadsABackwardsCounterAsAFreshStart(): Unit =
    val c = new WriteCounters
    c.record(1L, 5L, 900L)
    val before = c.stats(1L)
    c.clear()
    c.record(1L, 1L, 10L)
    assertEquals(WriteStats(1L, 1L, 1L, 10L), c.stats(1L).since(before))

  @Test def concurrentWritersLoseNothing(): Unit =
    val c = new WriteCounters
    val threads = (0 until 4).map { _ =>
      new Thread(() => (0 until 10000).foreach(_ => c.record(7L, 1L, 2L)))
    }
    threads.foreach(_.start()); threads.foreach(_.join())
    assertEquals(WriteStats(7L, 40000L, 40000L, 80000L), c.stats(7L))

  @Test def theTelemetryRuntimeOwnsTheCounters(): Unit =
    val t = new TelemetryRuntime(startWorker = false)
    t.writes.record(3L, 1L, 5L)
    assertEquals(1L, t.writes.stats(3L).statements)
    t.close()
