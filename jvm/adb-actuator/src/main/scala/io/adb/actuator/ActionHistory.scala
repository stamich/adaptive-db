package io.adb.actuator

/** When the actuator last acted on each target, and how often it had to take an action back.
  *
  * The gate reads it for the cooldowns. Keys are [[ActionTarget.key]]. Thread-safe.
  */
final class ActionHistory:
  /** Time of the latest action by target key. */
  private var actions: Map[String, Long] = Map.empty
  /** Time of the latest rollback by target key. */
  private var rollbackAt: Map[String, Long] = Map.empty
  /** Rollbacks so far by target key. */
  private var rollbackCount: Map[String, Int] = Map.empty

  /** Notes an action (of any kind) on `target`. */
  def recordAction(target: ActionTarget, atNanos: Long): Unit = synchronized {
    actions = actions.updated(target.key, atNanos)
  }

  /** Notes a rollback of `target`; it is also an action. */
  def recordRollback(target: ActionTarget, atNanos: Long): Unit = synchronized {
    actions = actions.updated(target.key, atNanos)
    rollbackAt = rollbackAt.updated(target.key, atNanos)
    rollbackCount = rollbackCount.updated(target.key, rollbackCount.getOrElse(target.key, 0) + 1)
  }

  /** Time of the latest action on `key`. */
  def lastAction(key: String): Option[Long] = synchronized(actions.get(key))

  /** Time of the latest rollback of `key`. */
  def lastRollback(key: String): Option[Long] = synchronized(rollbackAt.get(key))

  /** How many times `key` was rolled back. */
  def rollbacks(key: String): Int = synchronized(rollbackCount.getOrElse(key, 0))

  /** Forgets everything. */
  def clear(): Unit = synchronized {
    actions = Map.empty
    rollbackAt = Map.empty
    rollbackCount = Map.empty
  }
