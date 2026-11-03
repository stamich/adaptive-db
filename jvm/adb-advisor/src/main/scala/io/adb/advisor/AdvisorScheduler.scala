package io.adb.advisor

import java.util.concurrent.{Executors, ScheduledExecutorService, ScheduledFuture, ThreadFactory, TimeUnit}

/** Runs [[Advisor.evaluate]] on a timer, on one daemon thread of its own.
  *
  * The thread exists only while the scheduler is started, so a database that never turns the advisor
  * on carries no extra thread. `designSource` is called on that thread each time; it must return an
  * immutable [[PhysicalDesign]] (the gateway builds it from the catalog and the statistics
  * provider). A failure of one evaluation is passed to `onError` and does not stop the timer.
  *
  * @param advisor      the advisor to run
  * @param designSource builds the physical design for an evaluation
  * @param periodMillis time between evaluations
  * @param onError      receives exceptions thrown by an evaluation (default: ignore)
  */
final class AdvisorScheduler(
    advisor: Advisor,
    designSource: () => PhysicalDesign,
    periodMillis: Long,
    onError: Throwable => Unit = _ => ()
) extends AutoCloseable:
  require(periodMillis > 0L, "periodMillis must be positive")

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
        try advisor.evaluate(designSource())
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

  def close(): Unit = stop()
