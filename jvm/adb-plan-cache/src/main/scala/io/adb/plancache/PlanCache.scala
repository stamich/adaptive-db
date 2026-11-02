package io.adb.plancache

import java.util.concurrent.atomic.LongAdder

/** What a cached plan was valid for, read from the engine **before** the plan was made.
  *
  * Reading before planning is what makes the stamp safe: if DDL or `ANALYZE` lands while the
  * planner runs, the stamp is already older than the world and the entry is rejected on its first
  * lookup, never trusted.
  *
  * @param schemaEpoch catalog epoch (every DDL, indexes included, advances it)
  * @param statistics  statistics generation of each entity the plan reads, as `(entityId, generation)`
  */
final case class PlanStamp(schemaEpoch: Long, statistics: Vector[(Long, Long)]):
  /** True if the engine still reports exactly these generations. */
  def isCurrent(epoch: Long, generationOf: Long => Long): Boolean =
    epoch == schemaEpoch && statistics.forall((entity, gen) => generationOf(entity) == gen)

/** Result of a lookup. */
enum Lookup[+V] derives CanEqual:
  /** A valid cached plan. */
  case Hit(value: V)
  /** No entry. */
  case Miss
  /** An entry existed but the schema or statistics moved on; it has been dropped. */
  case Stale

/** Result of offering a freshly made plan. */
enum Offer derives CanEqual:
  /** Stored. */
  case Stored
  /** First sighting: remembered, not stored. */
  case Deferred
  /** Too heavy for its shard. */
  case Rejected

/** Counters of one cache, as a snapshot.
  *
  * @param hits      lookups answered from the cache
  * @param misses    lookups with no entry
  * @param stale     lookups that found an out-of-date entry
  * @param inserts   plans stored
  * @param deferred  plans not stored on their first sighting
  * @param rejected  plans too heavy to store
  * @param evictions entries pushed out by the budget
  * @param entries   entries held now
  * @param bytes     approximate weight held now
  */
final case class CacheStats(
    hits: Long,
    misses: Long,
    stale: Long,
    inserts: Long,
    deferred: Long,
    rejected: Long,
    evictions: Long,
    entries: Int,
    bytes: Long
):
  /** Hits over all lookups (0 if there were none). */
  def hitRatio: Double =
    val total = hits + misses + stale
    if total == 0 then 0.0 else hits.toDouble / total

/** Cache of planned statements, validated against the engine's generations on every hit.
  *
  * The cache never decides *what* is safe to cache or what the stamp contains; the caller does.
  * It guarantees: a returned plan's stamp equals the engine's current generations at lookup time;
  * the heap held stays within the budget; a statement is stored only after being offered twice.
  *
  * @param maxBytes   budget over all entries (approximate)
  * @param shards     lock shards
  * @param seenWindow number of recent statements the admission filter remembers
  * @param weigh      approximate weight of a value, excluding its key and stamp
  */
final class PlanCache[V](maxBytes: Long, shards: Int, seenWindow: Int, weigh: V => Long):
  /** Stored entry: the plan and what it is valid for. */
  private final case class Entry(value: V, stamp: PlanStamp)

  /** Entries by statement key. */
  private val lru = new BoundedLru[StatementKey, Entry](
    maxBytes,
    shards,
    (k, e) => k.weight + weigh(e.value) + 32L * e.stamp.statistics.size + 64L
  )
  /** Admission filter. */
  private val seen = new SeenFilter(seenWindow)
  private val hitCount, missCount, staleCount, insertCount, deferredCount, rejectedCount, evictionCount =
    new LongAdder()

  /** The plan for `key` if it is still valid for the engine's `epoch` and `generationOf`. */
  def lookup(key: StatementKey, epoch: Long, generationOf: Long => Long): Lookup[V] =
    lru.get(key) match
      case None =>
        missCount.increment()
        Lookup.Miss
      case Some(entry) if entry.stamp.isCurrent(epoch, generationOf) =>
        hitCount.increment()
        Lookup.Hit(entry.value)
      case Some(_) =>
        lru.remove(key)
        staleCount.increment()
        Lookup.Stale

  /** Offers a plan made under `stamp`; stored only if `key` was offered before. `value` is built only then. */
  def offer(key: StatementKey, value: => V, stamp: PlanStamp): Offer =
    if !seen.seenBefore(key) then
      deferredCount.increment()
      Offer.Deferred
    else
      val evicted = lru.put(key, Entry(value, stamp))
      if evicted < 0 then
        rejectedCount.increment()
        Offer.Rejected
      else
        insertCount.increment()
        evictionCount.add(evicted)
        Offer.Stored

  /** Drops `key` (used when the engine rejects its plan as outdated). */
  def invalidate(key: StatementKey): Unit = lru.remove(key)

  /** Drops every entry and forgets the admission history. */
  def clear(): Int =
    seen.clear()
    lru.clear()

  /** Snapshot of the counters. */
  def stats: CacheStats = CacheStats(
    hitCount.sum, missCount.sum, staleCount.sum, insertCount.sum, deferredCount.sum,
    rejectedCount.sum, evictionCount.sum, lru.size, lru.weight
  )
