package io.adb.plancache

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The weight-bounded LRU and the admission filter. */
final class BoundedLruTest:
  /** A cache of string values weighing their length. */
  private def lru(budget: Long) = new BoundedLru[Int, String](budget, 1, (_, v) => v.length.toLong)

  /** The least recently used entries go first when the budget is exceeded. */
  @Test def evictsLeastRecentlyUsed(): Unit =
    val c = lru(10)
    assertEquals(0, c.put(1, "aaaa"))
    assertEquals(0, c.put(2, "bbbb"))
    assertEquals(Some("aaaa"), c.get(1)) // 1 is now more recent than 2
    assertEquals(1, c.put(3, "cccc"))
    assertEquals(None, c.get(2))
    assertEquals(Some("aaaa"), c.get(1))
    assertEquals(8L, c.weight)

  /** An entry over the budget is refused and disturbs nothing. */
  @Test def tooHeavyIsRejected(): Unit =
    val c = lru(5)
    c.put(1, "aa")
    assertEquals(-1, c.put(2, "toolong"))
    assertEquals(Some("aa"), c.get(1))
    assertEquals(1, c.size)

  /** Replacing a key adjusts the weight; remove and clear release it. */
  @Test def weightIsTracked(): Unit =
    val c = lru(100)
    c.put(1, "aaaa"); c.put(1, "bb")
    assertEquals(2L, c.weight)
    assertTrue(c.remove(1)); assertFalse(c.remove(1))
    c.put(2, "x"); c.put(3, "y")
    assertEquals(2, c.clear())
    assertEquals(0L, c.weight)

  /** Many threads never break the bound or lose consistency. */
  @Test def concurrentUseKeepsTheBound(): Unit =
    val c = new BoundedLru[Int, String](400, 4, (_, v) => v.length.toLong)
    val threads = (0 until 8).map { t =>
      new Thread(() => (0 until 2000).foreach { i => c.put((i * 7 + t) % 300, "xxxxxxxx"); c.get(i % 300) })
    }
    threads.foreach(_.start()); threads.foreach(_.join())
    assertTrue(c.weight <= 400, s"weight ${c.weight}")

  /** The filter admits on the second sighting and forgets old keys. */
  @Test def seenFilterAdmitsOnSecondSighting(): Unit =
    val f = new SeenFilter(2)
    assertFalse(f.seenBefore("a"))
    assertTrue(f.seenBefore("a"))
    f.seenBefore("b"); f.seenBefore("c") // pushes "a" out
    assertFalse(f.seenBefore("a"))
