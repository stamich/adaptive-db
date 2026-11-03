package io.adb.advisor

import io.adb.model.*
import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.telemetry.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** End-to-end tests of one advisor over a scripted counter stream. */
final class AdvisorTest:
  private val second = 1_000_000_000L
  private val orders = 7L
  private val shape = QueryShape(Vector(orders), Vector(FieldRef(orders, 2L)), Vector.empty, Vector.empty, false, false)

  /** Cumulative counters after `n` runs, each 100 ms and reading 100 000 rows to return 10. */
  private def stats(n: Long): Vector[ShapeStats] =
    Vector(ShapeStats(shape.id, n, 0L, 0L, n * 100_000_000L, 100_000_000L, n * 10, n * 100_000, 1.0))

  private def design(indexes: Vector[IndexInfo] = Vector.empty) =
    PhysicalDesign.of(Seq(EntityDesign(orders, "orders", Map(1L -> "id", 2L -> "customer_id"), 100_000L, None, indexes)))

  /** An advisor whose counters and clock the test moves by hand. */
  private final class Script(windows: Int = 60):
    var runs = 0L
    var now = 0L
    val advisor = new Advisor(() => stats(runs), id => Option.when(id == shape.id)(shape), AdvisorConfig(windows = windows, confirmAfter = 2, withdrawAfter = 2), () => now)
    /** Runs `n` more queries, lets `seconds` pass and evaluates against `d`. */
    def step(n: Long, seconds: Long, d: PhysicalDesign = design()): AdvisorReport =
      runs += n; now += seconds * second; advisor.evaluate(d)

  /** The index recommendations of a report (the hot-query information is not under test here). */
  private def indexes(report: AdvisorReport): Vector[CreateIndex] = report.recommendations.collect { case c: CreateIndex => c }

  @Test def aSteadyWasteIsConfirmedAfterTheConfiguredEvaluations(): Unit =
    val s = new Script
    assertTrue(s.step(0, 0).recommendations.isEmpty, "the first evaluation is only a baseline")
    assertTrue(s.step(200, 20).recommendations.isEmpty, "seen once of two")
    val report = s.step(200, 20)
    assertEquals(Some("CREATE INDEX adv_orders_customer_id ON orders (customer_id)"), indexes(report).headOption.flatMap(_.sql))
    assertEquals(2, report.recommendations.size, "the index and the hot-query information")
    assertEquals(Vector(AuditKind.Confirmed, AuditKind.Confirmed), report.changes.map(_.kind))
    assertEquals(report.recommendations.head, indexes(report).head, "ordered by cost-benefit ratio, information last")
    assertEquals(400L, report.executions)

  @Test def creatingTheIndexWithdrawsTheAdviceAsApplied(): Unit =
    val s = new Script
    s.step(0, 0); s.step(200, 20); s.step(200, 20)
    val ix = IndexInfo(IndexId(1), "ix", EntityId(orders), Vector(IndexKeyColumn(FieldId(2), false)), Vector.empty, 1L, 1L, 1L)
    val report = s.step(200, 20, design(Vector(ix)))
    assertTrue(indexes(report).isEmpty)
    assertEquals(Vector(AuditKind.Applied), report.changes.map(_.kind))
    assertEquals(1, s.advisor.auditLog.entries.count(_.kind == AuditKind.Applied))

  @Test def aWorkloadThatStopsIsWithdrawnAfterTheConfiguredMisses(): Unit =
    // Two windows are kept, so two quiet evaluations push the traffic out of the model.
    val s = new Script(windows = 2)
    s.step(0, 0); s.step(200, 20); s.step(200, 20)
    assertEquals(1, indexes(s.step(0, 20)).size, "the busy window is still in the model")
    assertEquals(1, indexes(s.step(0, 20)).size, "found no more, but one miss of two")
    val report = s.step(0, 20)
    assertTrue(report.recommendations.isEmpty)
    assertTrue(report.changes.forall(_.kind == AuditKind.Withdrawn) && report.changes.nonEmpty)

  @Test def theAdvisorNeverNeedsAnythingButImmutableValues(): Unit =
    val s = new Script
    s.step(0, 0)
    val report = s.step(10, 10)
    assertEquals(Some(report), s.advisor.lastReport)
    s.advisor.reset()
    assertEquals(None, s.advisor.lastReport)

  @Test def itReadsAnActualTelemetryRuntime(): Unit =
    val runtime = new TelemetryRuntime(startWorker = false)
    val advisor = Advisor.forTelemetry(runtime)
    assertEquals(0, advisor.evaluate(design()).recommendations.size)
    runtime.close()
