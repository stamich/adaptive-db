package io.adb.telemetry

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The allocation-free id set behind the shape registry's `contains`. */
class LongIdSetTest:
  /** Added ids are found, others and the reserved id 0 are not. */
  @Test def addsAndFindsIds(): Unit =
    val set = new LongIdSet(8)
    assertFalse(set.contains(42L))
    assertTrue(set.add(42L))
    assertTrue(set.add(42L), "adding twice is harmless")
    assertTrue(set.contains(42L))
    assertFalse(set.contains(43L))
    assertFalse(set.add(0L), "0 is the gateway's 'no shape' value and is never stored")
    assertFalse(set.contains(0L))
    assertTrue(set.add(-7L) && set.contains(-7L), "negative ids work")

  /** Ids that start at the same slot are all kept (linear probing). */
  @Test def collidingIdsAreAllKept(): Unit =
    val set = new LongIdSet(64)
    // Multiples of a large power of two share the low bits that start the probe.
    val ids = (1L to 64L).map(_ << 40)
    ids.foreach(id => assertTrue(set.add(id)))
    ids.foreach(id => assertTrue(set.contains(id), s"id $id"))
    assertFalse(set.contains(65L << 40))

  /** The table holds the number of ids it was sized for, and `clear` empties it. */
  @Test def fillsToItsSizeAndClears(): Unit =
    val set = new LongIdSet(100)
    (1L to 100L).foreach(id => assertTrue(set.add(id * 7919L)))
    (1L to 100L).foreach(id => assertTrue(set.contains(id * 7919L)))
    set.clear()
    (1L to 100L).foreach(id => assertFalse(set.contains(id * 7919L)))

  /** Several threads adding overlapping ids end with exactly the union. */
  @Test def concurrentAddsKeepEveryId(): Unit =
    val set = new LongIdSet(4000)
    val threads = (0 until 4).map { t =>
      new Thread(() => (1L to 1000L).foreach(i => set.add(i * 31L + (if t % 2 == 0 then 0L else 1000000L))))
    }
    threads.foreach(_.start()); threads.foreach(_.join())
    (1L to 1000L).foreach { i =>
      assertTrue(set.contains(i * 31L))
      assertTrue(set.contains(i * 31L + 1000000L))
    }

  /** The point of the class: `contains` allocates nothing, unlike `ConcurrentHashMap[Long, _]`. */
  @Test def containsDoesNotAllocate(): Unit =
    val set = new LongIdSet(1024)
    (1L to 500L).foreach(i => set.add(i * 104729L))
    val threads = java.lang.management.ManagementFactory.getThreadMXBean.asInstanceOf[com.sun.management.ThreadMXBean]
    var hits = 0L
    var i = 0
    while i < 20000 do { if set.contains((i % 600 + 1) * 104729L) then hits += 1; i += 1 } // warm up
    val before = threads.getCurrentThreadAllocatedBytes
    i = 0
    while i < 200000 do { if set.contains((i % 600 + 1) * 104729L) then hits += 1; i += 1 }
    val allocated = threads.getCurrentThreadAllocatedBytes - before
    assertTrue(hits > 0)
    assertTrue(allocated < 1024L, s"contains allocated $allocated bytes over 200000 calls")
