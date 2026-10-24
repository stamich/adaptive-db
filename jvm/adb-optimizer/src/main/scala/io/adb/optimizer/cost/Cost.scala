package io.adb.optimizer.cost

/** Weights that turn a [[Cost]] into one comparable number.
  *
  * The units are deliberately simple: `cpu` counts row operations (a filter evaluation, a sort
  * comparison, an aggregate update), `io` counts 8 KiB pages of row data read from storage,
  * `hashRow` counts rows a hash join inserts into or probes against its hash table, `memory`
  * counts peak bytes held by blocking operators. Only ratios matter.
  *
  * The defaults are fitted to measured engine times by `scripts/fit-cost-weights.py` over the
  * `calibration.points` of the FFI benchmark (see docs/optimizer.md#calibration): reading a page
  * of rows (walking the primary index, decoding about 200 rows) costs a few thousand row
  * operations, which is why scan-heavy plans looked several times cheaper than they ran before
  * 2.3.0. The fit attributes no measurable time to hashing beyond that; a hashed row is kept at
  * one row operation so a hash join is never free.
  *
  * @param cpu     weight of one row operation
  * @param io      weight of one page of row data read
  * @param memory  weight of one byte held
  * @param hashRow weight of one row built into or probed against a hash table
  */
final case class CostWeights(
    cpu: Double = 1.0,
    io: Double = CostWeights.FittedIo,
    memory: Double = 0.0001,
    hashRow: Double = CostWeights.FittedHashRow
) derives CanEqual:
  require(
    Seq(cpu, io, memory, hashRow).forall(w => w >= 0 && !w.isInfinite && !w.isNaN),
    "cost weights must be finite and non-negative"
  )

/** Fitted weight values (relative to one row operation). */
object CostWeights:
  /** Weight of one page of row data read: 4,545 fitted over three runs on the reference
    * machine (scale factors 1 and 2), rounded. A page takes about 140 µs there.
    */
  val FittedIo: Double = 4500.0
  /** Weight of one hashed row; the fit finds no cost beyond one row operation. */
  val FittedHashRow: Double = 1.0

/** Estimated work of a plan or operator. Arithmetic saturates at [[Cost.Max]], so costs stay
  * finite and comparable however large the estimates get.
  *
  * @param cpu         row operations
  * @param io          pages read
  * @param memoryBytes peak bytes held by blocking operators
  * @param hashRows    rows inserted into or probed against hash tables
  */
final case class Cost(cpu: Double, io: Double, memoryBytes: Double, hashRows: Double = 0) derives CanEqual:
  /** Component-wise sum (memory adds up: blocking operators of one pipeline coexist). */
  def +(other: Cost): Cost =
    Cost.bounded(cpu + other.cpu, io + other.io, memoryBytes + other.memoryBytes, hashRows + other.hashRows)

  /** The weighted total. */
  def total(weights: CostWeights): Double =
    Cost.clamp(cpu * weights.cpu + io * weights.io + memoryBytes * weights.memory + hashRows * weights.hashRow)

/** Construction helpers of [[Cost]]. */
object Cost:
  /** Largest value of any component and of a total. */
  val Max: Double = 1e18
  /** No work. */
  val Zero: Cost = Cost(0, 0, 0)

  /** A cost with every component clamped to `[0, Max]` (NaN counts as `Max`). */
  def bounded(cpu: Double, io: Double, memoryBytes: Double, hashRows: Double = 0): Cost =
    Cost(clamp(cpu), clamp(io), clamp(memoryBytes), clamp(hashRows))

  /** One value clamped to `[0, Max]`. */
  def clamp(value: Double): Double = if value.isNaN then Max else value.max(0).min(Max)
