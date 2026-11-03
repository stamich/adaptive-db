package io.adb.telemetry

import java.util.concurrent.atomic.AtomicLongArray

/** A fixed-capacity set of non-zero `Long` ids that answers `contains` without allocating.
  *
  * [[QueryShapeRegistry]] keeps a `ConcurrentHashMap[Long, QueryShape]`, whose `containsKey` boxes
  * the `Long` key: about 24 bytes for every query the sampler looks at, even when the query is then
  * skipped. This set stores the ids in a plain `long` array (open addressing, linear probing), so
  * the sampler's "is this shape known" test touches no object at all.
  *
  * Ids are only ever added (or all dropped by [[clear]]); there is no single removal, which keeps
  * probing correct without tombstones. The table is sized for at most half occupancy, so a probe
  * sequence is short and always ends at an empty slot. Id `0` is the "no shape" value of the
  * gateway and is never stored.
  *
  * Safe for concurrent readers and writers: a slot goes from `0` to its id exactly once, by
  * compare-and-set, and a reader that misses a concurrent insert only sees "unknown", which makes
  * the sampler capture one extra query.
  *
  * @param maxIds the most ids the owner will add; the table is at least twice this size
  */
private[telemetry] final class LongIdSet(maxIds: Int):
  require(maxIds > 0, "maxIds must be positive")
  /** Slot count: a power of two, at least `2 * maxIds`. */
  private val capacity: Int =
    var n = 4
    while n < maxIds.toLong * 2 && n < (1 << 30) do n <<= 1
    n
  /** `capacity - 1`, to wrap an index with a bit-and. */
  private val mask: Int = capacity - 1
  /** The slots; `0` means empty. */
  private val slots = new AtomicLongArray(capacity)

  /** The first slot to probe for `id`: a multiplicative hash spreads the FNV ids evenly. */
  private def start(id: Long): Int = ((id * -7046029254386353131L) >>> 32).toInt & mask

  /** Whether `id` was added. Allocation free. */
  def contains(id: Long): Boolean =
    if id == 0L then false
    else
      var i = start(id)
      var steps = 0
      var found = false
      var done = false
      while !done && steps < capacity do
        val v = slots.get(i)
        if v == id then { found = true; done = true }
        else if v == 0L then done = true
        else { i = (i + 1) & mask; steps += 1 }
      found

  /** Adds `id`; false if it is `0` or the table is full (which the sizing makes impossible when the owner respects `maxIds`). */
  def add(id: Long): Boolean =
    if id == 0L then false
    else
      var i = start(id)
      var steps = 0
      var result = false
      var done = false
      while !done && steps < capacity do
        val v = slots.get(i)
        if v == id then { result = true; done = true }
        else if v == 0L then
          if slots.compareAndSet(i, 0L, id) then { result = true; done = true }
          // else another thread took the slot: look at it again, it may hold `id`
        else { i = (i + 1) & mask; steps += 1 }
      result

  /** Drops every id. */
  def clear(): Unit =
    var i = 0
    while i < capacity do { slots.set(i, 0L); i += 1 }
