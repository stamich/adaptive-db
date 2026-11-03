package io.adb.actuator

import io.adb.advisor.WorkloadWindow
import io.adb.telemetry.QueryShape

/** Finds advisor-owned indexes nobody has wanted for a long time.
  *
  * An index is **wanted** in a window if some shape that ran in it filters on, or joins through, the
  * index's leading column. (That is the column the advisor indexed for, whatever the plan finally
  * chose; the engine's per-index use counter is not exposed, so this is the evidence available.) It
  * counts one idle window for each window **in which the workload ran at all** without wanting it;
  * a window in which nothing ran says nothing about the index (the system was quiet, or telemetry was
  * off), so it neither counts nor resets. A window that wants the index resets its count.
  *
  * Only indexes handed to [[observe]] are tracked, and the controller hands over only advisor-owned
  * ones, so a user's index can never be reported idle.
  *
  * @param idleWindows windows without any use before an index is reported
  */
final class IdleDetector(idleWindows: Int):
  require(idleWindows >= 1, "idleWindows must be at least 1")

  /** Consecutive idle windows by lower-case index name. */
  private var idle: Map[String, Int] = Map.empty

  /** Folds one closed window into the counts.
    *
    * @param owned  the advisor's indexes
    * @param window the window that just closed
    * @param shapes looks a shape up by id
    * @return the indexes idle for at least `idleWindows` windows
    */
  def observe(owned: Vector[OwnedIndex], window: WorkloadWindow, shapes: Long => Option[QueryShape]): Vector[OwnedIndex] = synchronized {
    val ran = window.shapes.valuesIterator.exists(_.executions > 0L)
    val seen = window.shapes.keysIterator.flatMap(shapes(_)).toVector
    val next = owned.map { index =>
      val key = index.name.toLowerCase
      val before = idle.getOrElse(key, 0)
      val count =
        if !ran then before
        else if seen.exists(OwnedIndex.wants(index, _)) then 0
        else before + 1
      key -> count
    }.toMap
    idle = next
    owned.filter(i => next(i.name.toLowerCase) >= idleWindows)
  }

  /** Idle windows counted so far for `name`. */
  def idleCount(name: String): Int = synchronized(idle.getOrElse(name.toLowerCase, 0))

  /** Forgets `name` (it was dropped). */
  def forget(name: String): Unit = synchronized { idle -= name.toLowerCase }
