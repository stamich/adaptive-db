package io.adb.advisor

import java.util.concurrent.{Executors, ScheduledExecutorService, ScheduledFuture, ThreadFactory, TimeUnit}

/** Runs a periodic task on one daemon thread of its own: by default an [[Advisor.evaluate]], and since
  * 2.5.4 anything else the gateway hands it (the actuator's controller tick uses the same timer).
  *
  * The thread exists only while the scheduler is started, so a database that never turns the advisor
  * on carries no extra thread. The task runs on that thread each period. A failure of one run is
  * passed to `onError` and does not stop the timer.
  *
  * @param tick         the work of one period
  * @param periodMillis time between runs
  * @param onError      receives exceptions thrown by a run (default: ignore)
  */
final class AdvisorScheduler(
    tick: () => Unit,
    periodMillis: Long,
    onError: Throwable => Unit = _ => ()
) extends AutoCloseable:
  require(periodMillis > 0L, "periodMillis must be positive")

  /** A scheduler that evaluates `advisor` every period.
    *
    * @param advisor      the advisor to run
    * @param designSource builds the immutable [[PhysicalDesign]] for an evaluation; called on the timer thread
    * @param periodMillis time between evaluations
    */
  def this(advisor: Advisor, designSource: () => PhysicalDesign, periodMillis: Long) =
    this(() => { advisor.evaluate(designSource()); () }, periodMillis, _ => ())

  /** As above, with an error handler for failed evaluations. */
  def this(advisor: Advisor, designSource: () => PhysicalDesign, periodMillis: Long, onError: Throwable => Unit) =
    this(() => { advisor.evaluate(designSource()); () }, periodMillis, onError)

  /** The timer thread, created by [[start]]. */
  private var executor: Option[ScheduledExecutorService] = None
  /** The scheduled task, while running. */
  private var task: Option[ScheduledFuture[?]] = None

  /** Starts the timer; does nothing if it already runs. The first evaluation comes after one period. */
  def start(): Unit = synchronized {
    if task.isEmpty then
      val pool = Executors.newSingleThreadScheduledExecutor(new ThreadFactory:
        def newThread(r: Runnable): Thread =
          val t = new Thread(r, "adb-advisor")
          t.setDaemon(true)
          t)
      executor = Some(pool)
      val run: Runnable = () =>
        try tick()
        catch case e: Exception => onError(e)
      task = Some(pool.scheduleWithFixedDelay(run, periodMillis, periodMillis, TimeUnit.MILLISECONDS))
  }

  /** Stops the timer and its thread; does nothing if it is not running. */
  def stop(): Unit = synchronized {
    task.foreach(_.cancel(false))
    executor.foreach(_.shutdownNow())
    task = None
    executor = None
  }

  /** Whether the timer runs. */
  def running: Boolean = synchronized(task.isDefined)

  /** Same as [[stop]], so the scheduler can be used with `Using`. */
  def close(): Unit = stop()
