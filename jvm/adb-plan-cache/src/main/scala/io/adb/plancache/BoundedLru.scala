package io.adb.plancache

import java.util.LinkedHashMap
import java.util.Map as JMap

/** A least-recently-used map bounded by total **weight** (approximate bytes), not entry count.
  *
  * The map is split into independently locked shards by key hash so concurrent sessions rarely
  * contend. Each shard owns an equal share of the budget; an entry heavier than a shard's share is
  * rejected rather than flushing the shard.
  *
  * @param maxWeight total budget over all shards
  * @param shards    number of shards (at least 1)
  * @param weigh     weight of one value
  */
final class BoundedLru[K, V](maxWeight: Long, shards: Int, weigh: (K, V) => Long):
  require(maxWeight > 0, "cache budget must be positive")
  require(shards >= 1, "at least one shard")

  /** One lock-protected shard in access order. */
  private final class Shard(val budget: Long):
    /** Entries, least recently used first. */
    val map = new LinkedHashMap[K, (V, Long)](16, 0.75f, true)
    /** Sum of entry weights. */
    var used = 0L

  /** The shards. */
  private val parts = Array.fill(shards)(new Shard(math.max(1L, maxWeight / shards)))

  /** Shard of `key`. */
  private def shardOf(key: K): Shard = parts(Math.floorMod(key.hashCode, parts.length))

  /** The value for `key`, marking it recently used. */
  def get(key: K): Option[V] =
    val s = shardOf(key)
    s.synchronized(Option(s.map.get(key)).map(_._1))

  /** Stores `value`; returns how many other entries were evicted, or -1 if it is too heavy to store. */
  def put(key: K, value: V): Int =
    val s = shardOf(key)
    val w = weigh(key, value)
    if w > s.budget then -1
    else
      s.synchronized {
        val old = s.map.put(key, (value, w))
        if old != null then s.used -= old._2
        s.used += w
        var evicted = 0
        val it = s.map.entrySet.iterator
        while s.used > s.budget && it.hasNext do
          val e: JMap.Entry[K, (V, Long)] = it.next()
          if e.getKey != key then
            s.used -= e.getValue._2
            it.remove()
            evicted += 1
        evicted
      }

  /** Removes `key`; true if it was present. */
  def remove(key: K): Boolean =
    val s = shardOf(key)
    s.synchronized {
      val old = s.map.remove(key)
      if old != null then s.used -= old._2
      old != null
    }

  /** Removes everything; returns the number of entries dropped. */
  def clear(): Int = parts.map { s =>
    s.synchronized { val n = s.map.size; s.map.clear(); s.used = 0; n }
  }.sum

  /** Number of entries. */
  def size: Int = parts.map(s => s.synchronized(s.map.size)).sum

  /** Total weight held. */
  def weight: Long = parts.map(s => s.synchronized(s.used)).sum
