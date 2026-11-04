package io.adb.bench

import java.lang.management.ManagementFactory
import scala.jdk.CollectionConverters.*

/** What the JIT compiler and the garbage collector did around a measurement.
  *
  * A case measured while the JIT is still compiling it reports the compiler's work, not the code's.
  * The record makes that visible: `compileMsDuringMeasurement` should be (close to) zero, and
  * [[warm]] says whether it was small enough to trust the samples.
  *
  * @param supported                   whether the JVM reports compilation time at all
  * @param compiler                    name of the JIT compiler
  * @param compileMsBeforeWarmup       compilation time accumulated by the process before the case
  * @param compileMsDuringWarmup       compilation time during warm-up and calibration
  * @param compileMsDuringMeasurement  compilation time while the samples were taken
  * @param measurementMs               wall time of the measurement
  * @param gcCountDuringMeasurement    garbage collections while the samples were taken
  * @param gcMsDuringMeasurement       time spent in them
  */
final case class JitRecord(
    supported: Boolean,
    compiler: Option[String],
    compileMsBeforeWarmup: Long,
    compileMsDuringWarmup: Long,
    compileMsDuringMeasurement: Long,
    measurementMs: Double,
    gcCountDuringMeasurement: Long,
    gcMsDuringMeasurement: Long
):
  /** Whether compilation took under 1 % of the measurement: the code was compiled before timing. */
  def warm: Boolean = supported && compileMsDuringMeasurement * 100.0 <= measurementMs

  /** The record as JSON. */
  def json: String = Json.obj(
    "supported" -> Json.bool(supported),
    "compiler" -> Json.strOpt(compiler),
    "compile_ms_before_warmup" -> Json.num(compileMsBeforeWarmup),
    "compile_ms_during_warmup" -> Json.num(compileMsDuringWarmup),
    "compile_ms_during_measurement" -> Json.num(compileMsDuringMeasurement),
    "measurement_ms" -> Json.num(measurementMs),
    "gc_count_during_measurement" -> Json.num(gcCountDuringMeasurement),
    "gc_ms_during_measurement" -> Json.num(gcMsDuringMeasurement),
    "warm" -> Json.bool(warm)
  )

/** The raw outcome of [[Runner.measure]].
  *
  * @param plan                the plan used
  * @param iterationsPerSample invocations folded into one sample by the calibration
  * @param batchesNs           per batch, the samples in nanoseconds per invocation, in order
  * @param last                the result of the last invocation, so a caller can verify the work
  * @param jit                 the JIT and GC record of the measurement
  */
final case class Measured[T](
    plan: MeasurementPlan,
    iterationsPerSample: Long,
    batchesNs: Vector[Vector[Double]],
    last: T,
    jit: JitRecord
)

/** The measurement loop: warm-up, calibration, then batches of samples. */
object Runner:
  /** Keeps results observable, so the JIT cannot delete the work being timed. */
  private object Sink:
    /** The last result; volatile, so the write cannot be elided. */
    @volatile var value: Any = null

  /** Compilation time and GC counters, read before and after a phase. */
  private final case class Counters(compileMs: Long, gcCount: Long, gcMs: Long)

  /** Reads the JIT and GC counters. */
  private def counters(): Counters =
    val compilation = ManagementFactory.getCompilationMXBean
    val compile = if compilation != null && compilation.isCompilationTimeMonitoringSupported then compilation.getTotalCompilationTime else 0L
    val collectors = ManagementFactory.getGarbageCollectorMXBeans.asScala.toVector
    Counters(compile, collectors.map(_.getCollectionCount).filter(_ >= 0).sum, collectors.map(_.getCollectionTime).filter(_ >= 0).sum)

  /** Builds the [[JitRecord]] from the counters at the three phase boundaries. */
  private def record(start: Counters, afterWarmup: Counters, end: Counters, measurementNs: Long): JitRecord =
    val compilation = ManagementFactory.getCompilationMXBean
    JitRecord(
      supported = compilation != null && compilation.isCompilationTimeMonitoringSupported,
      compiler = Option(compilation).map(_.getName),
      compileMsBeforeWarmup = start.compileMs,
      compileMsDuringWarmup = afterWarmup.compileMs - start.compileMs,
      compileMsDuringMeasurement = end.compileMs - afterWarmup.compileMs,
      measurementMs = measurementNs / 1e6,
      gcCountDuringMeasurement = end.gcCount - afterWarmup.gcCount,
      gcMsDuringMeasurement = end.gcMs - afterWarmup.gcMs
    )

  /** Warms `body` up, calibrates the iterations of one sample, then takes `plan.batches` batches of
    * `plan.samplesPerBatch` samples. Every result goes through a volatile sink.
    */
  def measure[T](plan: MeasurementPlan)(body: => T): Measured[T] =
    val start = counters()
    var last = body
    Sink.value = last
    for _ <- 0 until plan.warmupInvocations do
      last = body
      Sink.value = last
    var iterations = 1L
    var calibrated = false
    while !calibrated do
      val began = System.nanoTime()
      var i = 0L
      while i < iterations do
        last = body
        Sink.value = last
        i += 1
      if System.nanoTime() - began >= plan.minSampleNs || iterations >= plan.maxIterations then calibrated = true
      else iterations = math.min(iterations * 2, plan.maxIterations)
    val afterWarmup = counters()
    val measuredFrom = System.nanoTime()
    val batches = Vector.fill(plan.batches) {
      Vector.fill(plan.samplesPerBatch) {
        val began = System.nanoTime()
        var i = 0L
        while i < iterations do
          last = body
          Sink.value = last
          i += 1
        (System.nanoTime() - began).toDouble / iterations
      }
    }
    val measuredNs = System.nanoTime() - measuredFrom
    Measured(plan, iterations, batches, last, record(start, afterWarmup, counters(), measuredNs))

  /** Like [[measure]], for work that must start from a fresh state each time: `setup` runs untimed
    * and returns the state, `run` consumes it and is timed. Each sample is exactly one `run`.
    */
  def measureWithSetup[S, T](plan: MeasurementPlan)(setup: => S)(run: S => T): Measured[T] =
    val start = counters()
    var last = run(setup)
    Sink.value = last
    for _ <- 0 until plan.warmupInvocations do
      last = run(setup)
      Sink.value = last
    val afterWarmup = counters()
    val measuredFrom = System.nanoTime()
    var timedNs = 0L
    val batches = Vector.fill(plan.batches) {
      Vector.fill(plan.samplesPerBatch) {
        val state = setup
        val began = System.nanoTime()
        last = run(state)
        val elapsed = System.nanoTime() - began
        Sink.value = last
        timedNs += elapsed
        elapsed.toDouble
      }
    }
    val measuredNs = System.nanoTime() - measuredFrom
    Measured(plan, 1L, batches, last, record(start, afterWarmup, counters(), measuredNs))
