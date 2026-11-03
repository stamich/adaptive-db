package io.adb.actuator

/** Keeps the advisor from creating and dropping the same index in a loop.
  *
  * A workload that wobbles around the thresholds could make the advisor build an index, find it does
  * not pay, drop it, and build it again, forever, each time bumping the schema epoch and flushing
  * the plan cache. The guard makes every rollback of the same target wait twice as long as the one
  * before: `rollbackCooldown`, then 2x, 4x, 8x, and 8x from then on. Creating the index again after
  * a rollback is therefore possible (the workload may have changed) but never a tight loop.
  */
object FlapGuard:
  /** Largest multiple of the rollback cooldown (three doublings). */
  val MaxFactor: Long = 8L

  /** The time a target must wait after its latest rollback.
    *
    * @param policy    gives the base `rollbackCooldownNanos`
    * @param rollbacks how many times the target has been rolled back (0 means never: no wait)
    */
  def rollbackCooldown(policy: Policy, rollbacks: Int): Long =
    if rollbacks <= 0 then 0L
    else policy.rollbackCooldownNanos * math.min(MaxFactor, 1L << math.min(rollbacks - 1, 3))
