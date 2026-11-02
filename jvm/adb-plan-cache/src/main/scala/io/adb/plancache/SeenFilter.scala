package io.adb.plancache

import java.util.LinkedHashMap
import java.util.Map as JMap

/** Admission filter: a statement is cached on its **second** sighting.
  *
  * One-off statements (ad-hoc exploration, generated literals) would otherwise fill the cache with
  * plans nobody reuses. The filter remembers only 32-bit hashes of recently seen keys in a bounded
  * FIFO. A hash collision admits a statement one sighting early, which costs nothing but a slot.
  *
  * @param limit    number of recent hashes remembered
  */
final class SeenFilter(limit: Int):
  require(limit > 0, "limit must be positive")

  /** Recent hashes in insertion order. */
  private val seen = new LinkedHashMap[Int, java.lang.Boolean](16, 0.75f, false):
    override def removeEldestEntry(eldest: JMap.Entry[Int, java.lang.Boolean]): Boolean = size > limit

  /** Records `key`; true if it was already recorded (and so deserves a cache slot). */
  def seenBefore(key: Any): Boolean = synchronized {
    val h = key.hashCode
    if seen.containsKey(h) then true
    else
      seen.put(h, java.lang.Boolean.TRUE)
      false
  }

  /** Forgets everything. */
  def clear(): Unit = synchronized(seen.clear())
