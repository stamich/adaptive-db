package io.adb.actuator

/** When the actuator may act and how it judges the result. All values are plain numbers so a
  * deployment can change them; the defaults are the ones of the 2.5.4 proposal.
  *
  * Times are in nanoseconds on the controller's clock; `windows` are workload windows, one per
  * controller tick (10 s with the default timer, so 6 windows are one minute).
  *
  * @param minExecutions     captured runs a shape needs, in the evidence and again in the verification, before it counts
  * @param minRatio          benefit/cost the advisor's score must reach to act (the advisor itself advises from 2.0)
  * @param cooldownNanos     least time between two actions on the same target
  * @param rollbackCooldownNanos least time before the advisor may recreate what it rolled back
  * @param verifyWindows     windows to wait after an action before judging it
  * @param minRowsReduction  rows read per execution must fall by at least this factor (2.0 = to half) for the action to be kept
  * @param timeTolerance     a kept index may not make the same shapes slower than this factor; time is noisy, so it only vetoes a clear loss
  * @param maxWriteSlowdown  tolerated rise of the mean write time on the entity (0.25 = 25 %), when the index does not pay for it
  * @param idleWindows       windows without any use before an advisor-owned index is dropped as idle
  * @param adoptByPrefix     whether indexes named `adv_*` found at start-up are taken over as advisor-owned
  */
final case class Policy(
    minExecutions: Long = 50L,
    minRatio: Double = 4.0,
    cooldownNanos: Long = Policy.minutes(30),
    rollbackCooldownNanos: Long = Policy.hours(6),
    verifyWindows: Int = 6,
    minRowsReduction: Double = 2.0,
    timeTolerance: Double = 2.0,
    maxWriteSlowdown: Double = 0.25,
    idleWindows: Int = 360,
    adoptByPrefix: Boolean = true
) derives CanEqual:
  require(minExecutions >= 1L, "minExecutions must be at least 1")
  require(minRatio >= 1.0, "minRatio must be at least 1")
  require(cooldownNanos >= 0L && rollbackCooldownNanos >= 0L, "cooldowns cannot be negative")
  require(verifyWindows >= 1, "verifyWindows must be at least 1")
  require(minRowsReduction > 1.0, "minRowsReduction must be above 1 (the index has to help)")
  require(timeTolerance >= 1.0, "timeTolerance must be at least 1")
  require(maxWriteSlowdown >= 0.0, "maxWriteSlowdown cannot be negative")
  require(idleWindows >= 1, "idleWindows must be at least 1")

/** Constants and helpers of [[Policy]]. */
object Policy:
  /** `n` minutes in nanoseconds. */
  def minutes(n: Long): Long = n * 60L * 1_000_000_000L

  /** `n` hours in nanoseconds. */
  def hours(n: Long): Long = n * 3600L * 1_000_000_000L

/** What the actuator may spend. An action that would exceed any limit is rejected before it has any effect.
  *
  * @param maxAdvisorIndexes  indexes the advisor may own at once
  * @param maxIndexBytes      total estimated size of the advisor's indexes
  * @param maxConcurrentBuilds index builds that may run at the same time
  * @param maxBuildRows       largest table the advisor builds an index on by itself; a bigger table needs a person
  * @param bytesPerEntry      estimated bytes per index entry (key plus row reference), used for the size estimate
  */
final case class Budget(
    maxAdvisorIndexes: Int = 8,
    maxIndexBytes: Long = 1L << 30,
    maxConcurrentBuilds: Int = 1,
    maxBuildRows: Long = 10_000_000L,
    bytesPerEntry: Long = 48L
) derives CanEqual:
  require(maxAdvisorIndexes >= 0, "maxAdvisorIndexes cannot be negative")
  require(maxIndexBytes >= 0L, "maxIndexBytes cannot be negative")
  require(maxConcurrentBuilds >= 1, "maxConcurrentBuilds must be at least 1")
  require(maxBuildRows >= 0L, "maxBuildRows cannot be negative")
  require(bytesPerEntry >= 1L, "bytesPerEntry must be at least 1")

  /** Estimated bytes of an index over `rows` rows. */
  def estimateBytes(rows: Long): Long = math.max(0L, rows) * bytesPerEntry
