package io.adb.bench

import scala.collection.mutable

/** The schema-3 report document, JVM side.
  *
  * The contract is the one of the Rust `report` module and `scripts/bench_validate.py`: the
  * required top-level keys, and per case the legacy timing keys plus a `timing` object. Timing keys
  * are in milliseconds.
  */
object ReportJson:
  /** The report schema this module writes. */
  val SchemaVersion = 3L

  /** The `timing` object of one case; `limitPercent` is the batch-MAD limit of the verdict. */
  def timing(stats: SampleStats, limitPercent: Double = SampleStats.DefaultStableBatchMadPercent): String =
    Json.obj(
      "median_of_medians_ms" -> Json.num(stats.medianNs / 1e6),
      "batch_medians_ms" -> Json.arr(stats.batchMediansNs.map(n => Json.num(n / 1e6))),
      "batch_mad_percent" -> Json.num(stats.batchMadPercent),
      "outlier_count" -> Json.num(stats.outlierIndices.size.toLong),
      "outlier_indices" -> Json.arr(stats.outlierIndices.map(i => Json.num(i.toLong))),
      "stability" -> Json.str(stats.stability(limitPercent).label),
      "stability_limit_percent" -> Json.num(limitPercent),
      "batches" -> Json.num(stats.batchMediansNs.size.toLong),
      "samples_per_batch" -> Json.num(stats.samplesPerBatch.toLong),
      "iterations_per_sample" -> Json.num(stats.iterationsPerSample),
      "quick" -> Json.bool(stats.quick)
    )

  /** The keys the reports of 2.5.0 to 2.5.4 had for a timed path (`p50_ms` is the headline). */
  def legacyTiming(stats: SampleStats): Vector[(String, String)] =
    def ms(ns: Double): String = Json.num(ns / 1e6)
    Vector(
      "p50_ms" -> ms(stats.medianNs),
      "p95_ms" -> ms(stats.p95Ns),
      "p99_ms" -> ms(stats.p99Ns),
      "mean_ms" -> ms(stats.meanNs),
      "min_ms" -> ms(stats.minNs),
      "max_ms" -> ms(stats.maxNs),
      "stddev_ms" -> ms(stats.stddevNs),
      "round_medians_ms" -> Json.arr(stats.batchMediansNs.map(ms))
    )

  /** The JSON of one case: legacy keys, the `timing` object, the JIT record, then `extra`, which
    * wins over the legacy keys. The case keeps the layout of the earlier reports.
    */
  def caseJson(
      stats: SampleStats,
      jit: Option[JitRecord] = None,
      extra: Seq[(String, String)] = Seq.empty,
      limitPercent: Double = SampleStats.DefaultStableBatchMadPercent
  ): String =
    val fields = mutable.LinkedHashMap.from(legacyTiming(stats))
    fields("timing") = timing(stats, limitPercent)
    jit.foreach(record => fields("jit") = record.json)
    extra.foreach((key, value) => fields(key) = value)
    Json.obj(fields.toSeq*)

  /** Units per second when one invocation does `units` units of work; zero for a zero median. */
  def ratePerSecond(stats: SampleStats, units: Double): Double =
    if stats.medianNs > 0.0 then units / (stats.medianNs / 1e9) else 0.0

/** A report under construction: set top-level keys, then [[Report.finish]]. */
final class Report(family: String, milestone: String, plan: MeasurementPlan, environment: Environment):
  /** Top-level keys in insertion order. */
  private val fields = mutable.LinkedHashMap[String, String]()
  fields("schema") = Json.num(ReportJson.SchemaVersion)
  fields("milestone") = Json.str(milestone)
  fields("family") = Json.str(family)
  fields("environment") = environment.json
  fields("runtime_before") = RuntimeSnapshot.capture().json
  fields("plan") = plan.json

  /** Sets (or replaces) a top-level key to an already-encoded JSON value. */
  def set(key: String, jsonValue: String): Unit = fields(key) = jsonValue

  /** Completes the document: captures `runtime_after` and returns the JSON text. */
  def finish(): String =
    fields("runtime_after") = RuntimeSnapshot.capture().json
    Json.obj(fields.toSeq*)
