package io.adb.actuator

import io.adb.advisor.*
import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.model.*
import io.adb.telemetry.*

/** A pretend database for controller tests: scripted counters, a clock and a design the fake
  * actuator changes, so the whole loop (advise, gate, act, measure, keep or undo) runs without an engine.
  *
  * One [[Load]] is one query shape with its own cost per run. [[step]] runs every active load, moves
  * the clock and ticks the controller, exactly as the timer would.
  */
final class Rig(
    val policy: Policy = Rig.testPolicy,
    val budget: Budget = Budget(),
    val confirmAfter: Int = 2
):
  val second = 1_000_000_000L
  /** The clock shared by the advisor and the controller. */
  var now = 0L

  /** One query shape and its cumulative counters. */
  final class Load(val shape: QueryShape, var rowsPerRun: Long, var nanosPerRun: Long, var qError: Double = 1.0):
    /** Cost per run without an index, restored when the index is dropped. */
    val baseRows: Long = rowsPerRun
    val baseNanos: Long = nanosPerRun
    var active = true
    /** Whether the shape filters on `field`, so an index on it would serve the shape. */
    def wants(field: Long): Boolean = shape.filterFields.exists(_.field == field)
    var executions, nanos, rowsRead, actual = 0L
    /** Runs the query `n` times. */
    def run(n: Long): Unit =
      executions += n; nanos += n * nanosPerRun; rowsRead += n * rowsPerRun; actual += n * 10
    def stats: ShapeStats = ShapeStats(shape.id, executions, 0L, 0L, nanos, nanosPerRun, actual, rowsRead, qError)

  /** The filter shape on `orders.customer_id` every test starts from. */
  val orders = 7L
  val ordersShape = QueryShape(Vector(orders), Vector(FieldRef(orders, 2L)), Vector.empty, Vector.empty, false, false)
  val load = new Load(ordersShape, rowsPerRun = 100_000L, nanosPerRun = 100_000_000L)
  var loads: Vector[Load] = Vector(load)

  /** Cumulative write counters of `orders`. */
  var writeStatements, writeNanos = 0L
  var writesPerStep = 0L
  var nanosPerWrite = 50_000L

  /** Indexes that exist now on `orders` (the user's and the advisor's). */
  var indexes: Vector[IndexInfo] = Vector.empty
  /** Whether a new index makes the load cheap (reads 10 rows, 1 ms). */
  var indexHelps = true
  /** What a write costs while an `adv_*` index exists, as a multiple of the base cost. */
  var writePenalty = 1.0
  private var nextIndexId = 1L

  /** An index on `orders` over `field`. */
  def indexInfo(name: String, field: Int): IndexInfo =
    val id = nextIndexId; nextIndexId += 1
    IndexInfo(IndexId(id), name, EntityId(orders), Vector(IndexKeyColumn(FieldId(field), descending = false)), Vector.empty, 1L, 1L, 1L)

  def design: PhysicalDesign =
    PhysicalDesign.of(Seq(
      EntityDesign(orders, "orders", Map(1L -> "id", 2L -> "customer_id", 3L -> "amount"), 100_000L, None, indexes),
      EntityDesign(8L, "items", Map(1L -> "id", 2L -> "sku"), 0L, None, Vector.empty)))

  val advisor = new Advisor(
    () => loads.map(_.stats),
    id => loads.find(_.shape.id == id).map(_.shape),
    AdvisorConfig(confirmAfter = confirmAfter, withdrawAfter = confirmAfter),
    () => now,
    Detectors.all,
    () => Vector(WriteStats(orders, writeStatements, writeStatements, writeNanos)))

  val fake = new FakeActuator
  /** The column an index action concerns (the controller's targets read `index:<entity>:<field>`). */
  private def fieldOf(action: Action): Int = action.target.key.split(':').last.toInt

  fake.onApply = action => action.kind match
    case ActionKind.CreateIndex =>
      indexes = indexes :+ indexInfo(action.indexName.get, fieldOf(action))
      if indexHelps then loads.filter(_.wants(fieldOf(action))).foreach { l => l.rowsPerRun = 10L; l.nanosPerRun = 1_000_000L }
      nanosPerWrite = (50_000L * writePenalty).toLong
    case ActionKind.DropIndex =>
      val dropped = indexes.find(_.name.equalsIgnoreCase(action.indexName.get))
      indexes = indexes.filterNot(_.name.equalsIgnoreCase(action.indexName.get))
      dropped.foreach(d => loads.filter(_.wants(d.columns.head.field.value)).foreach { l => l.rowsPerRun = l.baseRows; l.nanosPerRun = l.baseNanos })
      nanosPerWrite = 50_000L
    case ActionKind.Analyze => ()

  val controller = new AdaptiveController(advisor, fake, id => loads.find(_.shape.id == id).map(_.shape),
    AdaptiveConfig(policy, budget), () => now)

  /** Adds another load. By default it filters on `orders.customer_id` too (a second query the index would
    * serve, different shape); `wantsIndex = false` filters on `amount` instead, which the index does not
    * serve; `entity` picks another table.
    */
  def Load2(wantsIndex: Boolean = true, entity: Long = 7L, qError: Double = 1.0, rowsPerRun: Long = 100_000L): Load =
    val field = if wantsIndex && entity == 7L then 2L else 3L
    // hasLimit makes it a different shape from the first load even when it filters on the same column.
    val shape = QueryShape(Vector(entity), Vector(FieldRef(entity, field)), Vector.empty, Vector.empty, false, true)
    val l = new Load(shape, rowsPerRun, 100_000_000L, qError)
    loads = loads :+ l
    l

  /** Runs every active load `runs` times, lets `seconds` pass and ticks. */
  def step(runs: Long = 200L, seconds: Long = 20L): TickResult =
    loads.filter(_.active).foreach(_.run(runs))
    writeStatements += writesPerStep; writeNanos += writesPerStep * nanosPerWrite
    now += seconds * second
    controller.tick(design)

  /** Ticks `n` times with the default step. */
  def steps(n: Int): Unit = (0 until n).foreach(_ => step())

  /** Logged events so far. */
  def events: Vector[ActionEvent] = controller.log.entries.map(_.event)

object Rig:
  /** Small numbers so a scenario takes a handful of ticks. */
  val testPolicy: Policy = Policy(
    minExecutions = 50L, minRatio = 4.0, cooldownNanos = 60L * 1_000_000_000L,
    rollbackCooldownNanos = 600L * 1_000_000_000L, verifyWindows = 3, idleWindows = 4)
