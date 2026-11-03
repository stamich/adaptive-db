package io.adb.actuator

/** Limits how many index builds the actuator has running, and refuses new ones after shutdown.
  *
  * The controller runs one tick at a time, so in practice one build is in flight; the guard makes
  * the limit a property of the code instead of a habit of the caller, and gives shutdown a way to
  * stop work that has not begun. A build already inside the engine runs to its end (the engine
  * commits it atomically or not at all), so "abort" means: nothing new starts.
  *
  * @param maxConcurrent builds allowed at once (at least 1)
  */
final class BuildGuard(maxConcurrent: Int):
  require(maxConcurrent >= 1, "maxConcurrent must be at least 1")

  /** Builds running now. */
  private var running = 0
  /** Whether [[shutdown]] was called. */
  private var closed = false

  /** Reserves a build slot.
    *
    * @return `true` if the caller may build (and must call [[end]]); `false` if the limit is
    *         reached or the guard was shut down
    */
  def tryBegin(): Boolean = synchronized {
    if closed || running >= maxConcurrent then false
    else
      running += 1
      true
  }

  /** Releases a slot taken by [[tryBegin]]. */
  def end(): Unit = synchronized {
    if running > 0 then running -= 1
  }

  /** Runs `body` inside a slot; `None` if no slot was free. The slot is released even if `body` throws. */
  def guarded[A](body: => A): Option[A] =
    if !tryBegin() then None
    else
      try Some(body)
      finally end()

  /** Builds running now. */
  def inFlight: Int = synchronized(running)

  /** Refuses every later [[tryBegin]]. Does not interrupt a build that is running. */
  def shutdown(): Unit = synchronized { closed = true }

  /** Whether [[shutdown]] was called. */
  def isShutdown: Boolean = synchronized(closed)
