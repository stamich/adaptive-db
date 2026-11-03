package io.adb.plancache

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Validation of cached plans against the engine's generations. */
final class PlanCacheTest:
  /** A cache of strings. */
  private def cache(bytes: Long = 1 << 20) = new PlanCache[String](bytes, 2, 64, _.length.toLong)
  /** A key for `sql`. */
  private def key(sql: String) = StatementKey.of(sql, "cost").get
  /** Stamp over epoch 5 and entity 1 at generation 3. */
  private val stamp = PlanStamp(5, Vector(1L -> 3L))
  /** Generations: entity 1 is at 3 unless changed. */
  private def gens(g: Long): Long => Long = _ => g

  /** The second sighting stores, the third is a hit. */
  @Test def admitsOnSecondSightingAndThenHits(): Unit =
    val c = cache(); val k = key("SELECT a FROM t")
    assertEquals(Lookup.Miss, c.lookup(k, 5, gens(3)))
    assertEquals(Offer.Deferred, c.offer(k, "plan", stamp))
    assertEquals(Lookup.Miss, c.lookup(k, 5, gens(3)))
    assertEquals(Offer.Stored, c.offer(k, "plan", stamp))
    assertEquals(Lookup.Hit("plan"), c.lookup(k, 5, gens(3)))
    val s = c.stats
    assertEquals((1L, 2L, 1L, 1L), (s.hits, s.misses, s.inserts, s.deferred))
    assertTrue(s.hitRatio > 0.0)

  /** A moved schema epoch or statistics generation makes the entry stale and drops it. */
  @Test def staleEntriesAreDropped(): Unit =
    val c = cache(); val k = key("SELECT a FROM t")
    c.offer(k, "plan", stamp); c.offer(k, "plan", stamp)
    assertEquals(Lookup.Stale, c.lookup(k, 6, gens(3)))
    assertEquals(Lookup.Miss, c.lookup(k, 5, gens(3)))
    c.offer(k, "plan", stamp)
    assertEquals(Lookup.Stale, c.lookup(k, 5, gens(4)))
    assertEquals(2L, c.stats.stale)

  /** An entry over the budget is rejected; invalidate and clear remove entries. */
  @Test def rejectInvalidateAndClear(): Unit =
    val tiny = cache(100); val k = key("SELECT a FROM t")
    tiny.offer(k, "x" * 500, stamp)
    assertEquals(Offer.Rejected, tiny.offer(k, "x" * 500, stamp))
    val c = cache(); val k1 = key("SELECT a FROM t"); val k2 = key("SELECT b FROM t")
    for k <- Seq(k1, k2); _ <- 1 to 2 do c.offer(k, "p", stamp)
    c.invalidate(k1)
    assertEquals(Lookup.Miss, c.lookup(k1, 5, gens(3)))
    assertEquals(1, c.clear())
    assertEquals(0, c.stats.entries)
    assertEquals(Offer.Deferred, c.offer(k2, "p", stamp)) // history was forgotten too

  /** The budget holds under a flood of distinct statements. */
  @Test def budgetHoldsUnderFlood(): Unit =
    val c = cache(20000)
    for i <- 0 until 2000; _ <- 1 to 2 do c.offer(key(s"SELECT a FROM t WHERE a = $i"), "p" * 50, stamp)
    assertTrue(c.stats.bytes <= 20000)
    assertTrue(c.stats.evictions > 0)
