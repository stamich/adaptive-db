package io.adb.benchmark

import io.adb.bench.*
import io.adb.benchmark.FamilyCommon.*
import io.adb.ffm.{NativeDatabase, NativeQuery}
import io.adb.logical.{Binder, BoundSelect}
import io.adb.physical.PlanJsonEncoder
import io.adb.sql.SqlParser

import java.util.Optional

/** The `ffi` family (Milestone 2.5.4.1): the cost of crossing from the JVM to the engine.
  *
  * * **Calls** a cache hit makes before it runs the plan: the schema epoch, the statistics
  *   generation of one table and the commit head. They are the floor under any statement.
  * * **Prepared plans**: a plan already encoded as JSON, executed and drained through Java FFM, for a
  *   point lookup and for the relational query. Comparing these with the `sql-path` family's gateway
  *   cases isolates the cost of parsing, binding and planning from the cost of the engine and the
  *   boundary.
  * * **Profile**: fetching the execution profile document that telemetry needs.
  */
object FfiFamily:
  /** Case names, so a skipped run lists them. */
  private val Names = Vector(
    "native_call_schema_epoch", "native_call_statistics_generation", "native_call_latest_commit_seq",
    "ffi_execute_point_lookup", "ffi_execute_relational", "ffi_profile_fetch"
  )

  /** Runs the family and returns the report JSON. */
  def run(settings: FamilySettings): String =
    val plan = cpuPlan
    val cases = (settings.dataDir, settings.nativeLib) match
      case (Some(data), Some(lib)) => engineCases(data, lib, plan)
      case _ => Names.map(_ -> skipped("no --data and --native-lib given"))
    finish("ffi", plan, settings, Seq.empty, cases)

  /** The cases, against the real engine. */
  private def engineCases(data: java.nio.file.Path, lib: java.nio.file.Path, plan: MeasurementPlan): Vector[(String, String)] =
    val (native, db) = openGateway(data, lib)
    try
      val entityId = db.catalog.entity("account").get.id.value
      def prepared(sql: String): String =
        val bound = new Binder(db.catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
        val plans = db.plan(bound)
        PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch)
      val point = prepared(GatewayPointSql)
      val relational = prepared(RelationalSql)
      Vector(
        "native_call_schema_epoch" -> timed(plan)(native.currentSchemaEpoch()),
        "native_call_statistics_generation" -> timed(plan)(native.statisticsGeneration(entityId)),
        "native_call_latest_commit_seq" -> timed(plan)(native.latestCommitSeq()),
        "ffi_execute_point_lookup" -> timed(plan)(drain(native, point)),
        "ffi_execute_relational" -> timed(plan)(drain(native, relational)),
        "ffi_profile_fetch" -> timed(plan)(profile(native, point))
      )
    finally native.close()

  /** Executes a prepared plan and counts the rows. */
  private def drain(native: NativeDatabase, json: String): Long =
    val query = native.execute(json, Optional.empty[java.lang.Long]())
    try rows(query) finally query.close()

  /** Executes a prepared plan, drains it and returns the length of its profile document. */
  private def profile(native: NativeDatabase, json: String): Long =
    val query = native.execute(json, Optional.empty[java.lang.Long]())
    try rows(query) + query.profileJson().length finally query.close()

  /** Rows in the rest of the stream of `query`. */
  private def rows(query: NativeQuery): Long =
    var count = 0L
    var next = query.nextBatch()
    while next.isPresent do
      count += next.get().rowCount()
      next = query.nextBatch()
    count
