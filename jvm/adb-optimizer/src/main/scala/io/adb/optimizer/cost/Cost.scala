package io.adb.optimizer.cost

/** Weights that turn a [[Cost]] into one comparable number.
  *
  * The units are deliberately simple: `cpu` counts row operations (one row through a scan,
  * one hash probe, ...), `io` counts 8 KiB pages read, `memory` counts peak bytes held by
  * blocking operators. Only ratios matter; `ProfileCalibration` in the benchmark compares the
  * model with measured runtimes.
  *
  * @param cpu    weight of one row operation
  * @param io     weight of one page read
  * @param memory weight of one byte held
  */
final case class CostWeights(cpu: Double = 1.0, io: Double = 8.0, memory: Double = 0.0001) derives CanEqual:
  require(Seq(cpu, io, memory).forall(w => w >= 0 && !w.isInfinite && !w.isNaN), "cost weights must be finite and non-negative")

/** Estimated work of a plan or operator. Arithmetic saturates at [[Cost.Max]], so costs stay
  * finite and comparable however large the estimates get.
  *
  * @param cpu         row operations
  * @param io          pages read
  * @param memoryBytes peak bytes held by blocking operators
  */
final case class Cost(cpu: Double, io: Double, memoryBytes: Double) derives CanEqual:
  /** Component-wise sum (memory adds up: blocking operators of one pipeline coexist). */
  def +(other: Cost): Cost = Cost.bounded(cpu + other.cpu, io + other.io, memoryBytes + other.memoryBytes)

  /** The weighted total. */
  def total(weights: CostWeights): Double =
    Cost.clamp(cpu * weights.cpu + io * weights.io + memoryBytes * weights.memory)

/** Construction helpers of [[Cost]]. */
object Cost:
  /** Largest value of any component and of a total. */
  val Max: Double = 1e18
  /** No work. */
  val Zero: Cost = Cost(0, 0, 0)

  /** A cost with every component clamped to `[0, Max]` (NaN counts as `Max`). */
  def bounded(cpu: Double, io: Double, memoryBytes: Double): Cost = Cost(clamp(cpu), clamp(io), clamp(memoryBytes))

  /** One value clamped to `[0, Max]`. */
  def clamp(value: Double): Double = if value.isNaN then Max else value.max(0).min(Max)
