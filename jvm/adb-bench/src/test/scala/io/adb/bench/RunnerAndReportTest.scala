package io.adb.bench

import java.nio.file.Files
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the runner, the environment record and the report builder. */
final class RunnerAndReportTest:
  /** A plan that finishes quickly in tests. */
  private val Tiny = MeasurementPlan(warmupInvocations = 2, minSampleNs = 0L, maxIterations = 1L, batches = 3, samplesPerBatch = 4)

  /** The runner takes the planned number of samples and runs the warm-up first. */
  @Test def measureFollowsThePlan(): Unit =
    var calls = 0
    val measured = Runner.measure(Tiny) { calls += 1; calls }
    assertEquals(3, measured.batchesNs.size)
    assertTrue(measured.batchesNs.forall(_.size == 4))
    assertEquals(1L, measured.iterationsPerSample)
    // 1 first call + 2 warm-ups + 1 calibration pass + 12 samples
    assertEquals(16, calls)
    assertEquals(16, measured.last)

  /** Calibration folds invocations until a sample lasts the minimum. */
  @Test def calibrationDoublesIterations(): Unit =
    val plan = Tiny.copy(minSampleNs = 2_000_000L, maxIterations = 1L << 20)
    val measured = Runner.measure(plan) { Thread.sleep(0, 200_000); 1 }
    assertTrue(measured.iterationsPerSample > 1, s"iterations ${measured.iterationsPerSample}")

  /** Setup is not timed, and the state reaches the timed run. */
  @Test def setupIsNotTimed(): Unit =
    val measured = Runner.measureWithSetup(Tiny.copy(warmupInvocations = 1)) { Thread.sleep(15); 21 }(state => state * 2)
    assertEquals(42, measured.last)
    assertEquals(1L, measured.iterationsPerSample)
    assertTrue(measured.batchesNs.flatten.forall(_ < 5e6), "the 15 ms setup was timed")

  /** The JIT record is filled and serialises with every key. */
  @Test def jitRecordIsReported(): Unit =
    val record = Runner.measure(Tiny)(1).jit
    assertTrue(record.measurementMs >= 0.0)
    for key <- Seq("compile_ms_during_warmup", "compile_ms_during_measurement", "gc_count_during_measurement", "warm") do
      assertTrue(record.json.contains("\"" + key + "\""), key)

  /** `warm` needs compilation time to be under 1 % of the measurement. */
  @Test def warmMeansLittleCompilation(): Unit =
    val base = JitRecord(true, Some("HotSpot"), 100L, 50L, 0L, 1000.0, 0L, 0L)
    assertTrue(base.warm)
    assertFalse(base.copy(compileMsDuringMeasurement = 30L).warm)
    assertFalse(base.copy(supported = false).warm)

  /** The governor is classified as the Rust side classifies it. */
  @Test def governorIsClassified(): Unit =
    assertEquals(GovernorClass.Performance, GovernorClass.fromGovernor(Some("performance\n")))
    assertEquals(GovernorClass.Other("schedutil"), GovernorClass.fromGovernor(Some("schedutil")))
    assertEquals(GovernorClass.Unknown, GovernorClass.fromGovernor(None))
    assertEquals(GovernorClass.Unknown, GovernorClass.fromGovernor(Some("  ")))

  /** The runtime snapshot reads a fake tree; missing files are `null`, never an error. */
  @Test def runtimeSnapshotReadsAFakeTree(): Unit =
    val root = Files.createTempDirectory("adb-bench-root")
    val cpufreq = Files.createDirectories(root.resolve("sys/devices/system/cpu/cpu0/cpufreq"))
    Files.writeString(cpufreq.resolve("scaling_cur_freq"), "3400000\n")
    val zone = Files.createDirectories(root.resolve("sys/class/thermal/thermal_zone0"))
    Files.writeString(zone.resolve("temp"), "61500\n")
    Files.createDirectories(root.resolve("proc"))
    Files.writeString(root.resolve("proc/loadavg"), "0.50 0.75 1.00 1/200 123\n")
    val snapshot = RuntimeSnapshot.capture(root)
    assertEquals(Some(3400000L), snapshot.cpuFrequencyKhz)
    assertEquals(Some(61.5), snapshot.maxThermalC)
    assertEquals(Some(Vector(0.5, 0.75, 1.0)), snapshot.loadavg)
    val empty = RuntimeSnapshot.capture(Files.createTempDirectory("adb-bench-empty"))
    assertEquals(RuntimeSnapshot(None, None, None), empty)
    assertTrue(empty.json.contains("\"cpu_frequency_khz\":null"))

  /** The environment reads the governor and the CPU from the tree and the JVM from the process. */
  @Test def environmentRecordsMachineAndJvm(): Unit =
    val root = Files.createTempDirectory("adb-bench-env")
    val cpufreq = Files.createDirectories(root.resolve("sys/devices/system/cpu/cpu0/cpufreq"))
    Files.writeString(cpufreq.resolve("scaling_governor"), "powersave\n")
    Files.createDirectories(root.resolve("proc"))
    Files.writeString(root.resolve("proc/cpuinfo"), "processor\t: 0\nmodel name\t: Test CPU @ 1GHz\n")
    val env = Environment.capture(root)
    assertEquals(Some("powersave"), env.governor)
    assertEquals(Some("Test CPU @ 1GHz"), env.cpuModel)
    assertEquals(GovernorClass.Other("powersave"), env.governorClass)
    assertTrue(env.pid > 0)
    assertTrue(env.json.contains("\"fresh_fork\""))
    assertTrue(env.json.contains("\"governor\":\"powersave\""))

  /** JSON strings are escaped and non-finite numbers become `null`. */
  @Test def jsonEscapesAndHandlesNonFinite(): Unit =
    assertEquals("\"a\\\"b\\\\c\\n\"", Json.str("a\"b\\c\n"))
    assertEquals("null", Json.num(Double.NaN))
    assertEquals("null", Json.num(Double.PositiveInfinity))
    assertEquals("3", Json.num(3.0))
    assertEquals("2.5", Json.num(2.5))
    assertEquals("{\"a\":1,\"b\":[true]}", Json.obj("a" -> "1", "b" -> Json.arr(Seq(Json.bool(true)))))

  /** A case carries the legacy keys and a timing object with every required key. */
  @Test def caseKeepsLegacyKeysAndAddsTiming(): Unit =
    val stats = SampleStats.fromBatches(Seq(Seq(1e6, 1.1e6), Seq(1e6, 0.9e6), Seq(1e6, 1e6)), 4, quick = false)
    val json = ReportJson.caseJson(stats, extra = Seq("checksum" -> "42", "p50_ms" -> "-1"))
    for key <- Seq("p50_ms", "p95_ms", "p99_ms", "mean_ms", "min_ms", "max_ms", "stddev_ms", "round_medians_ms", "timing", "checksum") do
      assertTrue(json.contains("\"" + key + "\":"), key)
    for key <- Seq("median_of_medians_ms", "batch_medians_ms", "batch_mad_percent", "outlier_count", "outlier_indices",
        "stability", "batches", "samples_per_batch", "iterations_per_sample", "quick") do
      assertTrue(json.contains("\"" + key + "\":"), key)
    assertTrue(json.contains("\"p50_ms\":-1"), "extra keys win over the legacy ones")
    assertTrue(json.contains("\"iterations_per_sample\":4"))

  /** The report has the required top-level keys, in order, and a runtime snapshot at the end. */
  @Test def reportHasTheRequiredKeys(): Unit =
    val report = Report("test", "2.5.4.1", Tiny, Environment.capture(Files.createTempDirectory("adb-bench-r")))
    report.set("cases", Json.obj())
    val json = report.finish()
    for key <- Seq("schema", "milestone", "family", "environment", "runtime_before", "plan", "cases", "runtime_after") do
      assertTrue(json.contains("\"" + key + "\":"), key)
    assertTrue(json.startsWith("{\"schema\":3,\"milestone\":\"2.5.4.1\",\"family\":\"test\""))

  /** Rates divide by the median and survive a zero median. */
  @Test def rateHandlesZero(): Unit =
    val stats = SampleStats.fromBatches(Seq(Seq(1e6)), 1, quick = true)
    assertEquals(1000.0, ReportJson.ratePerSecond(stats, 1.0), 1e-9)
    val zero = SampleStats.fromBatches(Seq(Seq(0.0)), 1, quick = true)
    assertEquals(0.0, ReportJson.ratePerSecond(zero, 1.0), 0.0)

  /** Interleaved bodies each get every batch, and the visiting order rotates. */
  @Test def interleavedVisitsEveryBodyInEveryBatch(): Unit =
    val order = scala.collection.mutable.ArrayBuffer.empty[String]
    val plan = MeasurementPlan(warmupInvocations = 0, minSampleNs = 0L, maxIterations = 1L, batches = 3, samplesPerBatch = 1)
    val results = Runner.interleaved(plan)(Seq(
      "a" -> (() => { order += "a"; 1 }),
      "b" -> (() => { order += "b"; 2 }),
      "c" -> (() => { order += "c"; 3 })
    ))
    assertEquals(Vector("a", "b", "c"), results.map(_._1))
    assertEquals(Vector(1, 2, 3), results.map(_._2.last))
    assertTrue(results.forall(_._2.batchesNs.size == 3))
    // the timed part of the order: warm-up (1 each) + calibration (1 each) come first, then rotated batches
    val timed = order.drop(6).toVector
    assertEquals(Vector("a", "b", "c", "b", "c", "a", "c", "a", "b"), timed)

  /** A slower body measures slower, whatever its position. */
  @Test def interleavedKeepsBodiesApart(): Unit =
    val plan = MeasurementPlan(warmupInvocations = 1, minSampleNs = 0L, maxIterations = 1L, batches = 3, samplesPerBatch = 2)
    val results = Runner.interleaved(plan)(Seq("fast" -> (() => 1), "slow" -> (() => { Thread.sleep(5); 2 })))
    val fast = SampleStats.fromMeasured(results(0)._2).medianNs
    val slow = SampleStats.fromMeasured(results(1)._2).medianNs
    assertTrue(slow > fast * 10, s"fast=$fast slow=$slow")
