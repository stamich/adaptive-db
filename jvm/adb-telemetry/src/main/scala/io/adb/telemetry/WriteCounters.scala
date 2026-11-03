package io.adb.telemetry

import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.LongAdder

/** What the writes to one entity have cost so far (an immutable snapshot).
  *
  * @param entityId   the entity written
  * @param statements `INSERT`, `UPDATE` and `DELETE` statements that completed
  * @param rows       rows those statements changed
  * @param totalNanos wall-clock time of those statements
  */
final case class WriteStats(entityId: Long, statements: Long, rows: Long, totalNanos: Long):
  /** Mean elapsed nanoseconds per statement; 0 when there were none. */
  def meanNanos: Double = if statements == 0 then 0.0 else totalNanos.toDouble / statements

  /** The writes made between `earlier` and this snapshot, or this snapshot itself if a counter went
    * backwards (the counters were reset, so `earlier` no longer describes the same stream).
    */
  def since(earlier: WriteStats): WriteStats =
    if statements < earlier.statements || rows < earlier.rows || totalNanos < earlier.totalNanos then this
    else WriteStats(entityId, statements - earlier.statements, rows - earlier.rows, totalNanos - earlier.totalNanos)

/** Cumulative write counters per entity (2.5.4).
  *
  * The advisor of 2.5.3 saw only queries, so the price an index puts on writes was a flat guess.
  * These counters give the actuator a measurement: how many write statements an entity took, how
  * many rows they changed and how long they ran.
  *
  * **Hot path.** [[record]] is called once per finished write while telemetry is on. It does one
  * map lookup and three `LongAdder` increments, allocates nothing for an entity seen before and
  * never blocks. When telemetry is off the caller does not call it at all.
  */
final class WriteCounters:
  /** The three adders of one entity; separate adders so concurrent writers do not contend on one cell. */
  private final class Cell:
    val statements, rows, nanos = new LongAdder()

  /** Counters by entity id. */
  private val cells = new ConcurrentHashMap[Long, Cell]()

  /** Counts one completed write statement.
    *
    * @param entityId     entity that was written
    * @param rows         rows the statement changed
    * @param elapsedNanos wall-clock time of the statement
    */
  def record(entityId: Long, rows: Long, elapsedNanos: Long): Unit =
    var cell = cells.get(entityId)
    if cell == null then
      val fresh = new Cell
      val previous = cells.putIfAbsent(entityId, fresh)
      cell = if previous == null then fresh else previous
    cell.statements.increment()
    cell.rows.add(rows)
    cell.nanos.add(elapsedNanos)

  /** The counters of one entity, or zeroes if it has not been written. */
  def stats(entityId: Long): WriteStats =
    val cell = cells.get(entityId)
    if cell == null then WriteStats(entityId, 0L, 0L, 0L)
    else WriteStats(entityId, cell.statements.sum(), cell.rows.sum(), cell.nanos.sum())

  /** Snapshot of every entity written so far, ordered by entity id. */
  def all(): Vector[WriteStats] =
    val ids = cells.keySet().toArray(new Array[java.lang.Long](0)).map(_.longValue).sorted
    ids.iterator.map(stats).toVector

  /** Forgets every counter (telemetry was reset). */
  def clear(): Unit = cells.clear()
