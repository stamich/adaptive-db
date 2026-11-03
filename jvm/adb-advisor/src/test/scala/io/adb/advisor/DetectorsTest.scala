package io.adb.advisor

import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.model.*
import io.adb.statistics.{EntityStatistics, TableStatistics}
import io.adb.telemetry.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the four detectors over hand-built workloads. */
final class DetectorsTest:
  private val second = 1_000_000_000L
  private val orders = 7L
  private val customers = 8L

  private val ordersByCustomer = QueryShape(Vector(orders), Vector(FieldRef(orders, 2L)), Vector.empty, Vector.empty, false, false)
  private val joinShape = QueryShape(
    Vector(orders, customers), Vector.empty,
    Vector(JoinEdge(orders, 2L, customers, 1L)), Vector.empty, false, false
  )
  private val known = Map(ordersByCustomer.id -> ordersByCustomer, joinShape.id -> joinShape)

  private def design(rows: Long = 1_000_000L, indexes: Vector[IndexInfo] = Vector.empty, stats: Option[EntityStatistics] = None) =
    PhysicalDesign.of(Seq(
      EntityDesign(orders, "orders", Map(1L -> "id", 2L -> "customer_id"), rows, stats, indexes),
      EntityDesign(customers, "customers", Map(1L -> "id", 2L -> "name"), 1000L, None, Vector.empty)
    ))

  /** A workload with one entry per `(shape, executions, nanos, rowsRead, rowsReturned, qError)`. */
  private def workload(entries: (QueryShape, Long, Long, Long, Long, Double)*): WorkloadSnapshot =
    val deltas = entries.map((s, n, nanos, read, ret, q) => s.id -> ShapeDelta(s.id, n, 0L, nanos, ret, read, q)).toMap
    WorkloadSnapshot(Vector(WorkloadWindow(0L, 60 * second, deltas)), deltas)

  private def ctx(w: WorkloadSnapshot, d: PhysicalDesign = design()) =
    DetectorContext(w, known.get, d, CostBenefitModel(minExecutions = 10L), DetectorConfig())

  @Test def scanAmplificationRecommendsAnIndexOnTheFilterColumn(): Unit =
    val found = ScanAmplificationDetector.detect(ctx(workload((ordersByCustomer, 500L, 30 * second, 500_000_000L, 5_000L, 1.0))))
    val index = found.collect { case i: CreateIndex => i }.head
    assertEquals("CREATE INDEX adv_orders_customer_id ON orders (customer_id)", index.sql.get)
    assertTrue(index.score.get.ratio >= 2.0)
    assertEquals(Vector(ordersByCustomer.id), index.shapeIds)

  @Test def anExistingLeadingIndexSuppressesTheCandidate(): Unit =
    val ix = IndexInfo(IndexId(1), "ix", EntityId(orders), Vector(IndexKeyColumn(FieldId(2), false)), Vector.empty, 1L, 1L, 1L)
    val w = workload((ordersByCustomer, 500L, 30 * second, 500_000_000L, 5_000L, 1.0))
    assertTrue(ScanAmplificationDetector.detect(ctx(w, design(indexes = Vector(ix)))).isEmpty)

  @Test def aSelectiveShapeOrAThinOneIsNotAmplified(): Unit =
    assertTrue(ScanAmplificationDetector.detect(ctx(workload((ordersByCustomer, 500L, 30 * second, 1000L, 900L, 1.0)))).isEmpty)
    assertTrue(ScanAmplificationDetector.detect(ctx(workload((ordersByCustomer, 3L, 30 * second, 500_000_000L, 5L, 1.0)))).isEmpty)

  @Test def anIndexWhoseBuildOutweighsTheBenefitIsNotRecommended(): Unit =
    // 20 runs of 1 ms: a few ms saved per hour never pay for scanning a million rows.
    val w = workload((ordersByCustomer, 20L, 20_000_000L, 20_000L, 20L, 1.0))
    assertTrue(ScanAmplificationDetector.detect(ctx(w)).isEmpty)

  @Test def aNeverAnalyzedTableIsSizedByWhatTheScansRead(): Unit =
    // Row count 0 (never analyzed): without a floor the build would cost one row and anything would pass.
    val dear = workload((ordersByCustomer, 20L, 20_000_000L, 100_000_000L, 20L, 1.0))
    assertTrue(ScanAmplificationDetector.detect(ctx(dear, design(rows = 0L))).isEmpty, "5 million rows scanned per run: the build costs more than 1 ms runs can repay")
    val big = workload((ordersByCustomer, 500L, 30 * second, 500_000_000L, 5_000L, 1.0))
    assertEquals(1, ScanAmplificationDetector.detect(ctx(big, design(rows = 0L))).size)

  @Test def hotQueryReportsOnlyShapesWithALargeShareOfTime(): Unit =
    val w = workload((ordersByCustomer, 100L, 90 * second, 100L, 100L, 1.0), (joinShape, 100L, 5 * second, 100L, 100L, 1.0))
    val found = HotQueryDetector.detect(ctx(w))
    assertEquals(1, found.size)
    assertTrue(found.head.summary.contains("orders"), found.head.summary)
    assertTrue(found.head.summary.contains("95%"), found.head.summary)

  @Test def hotJoinNamesTheUnindexedJoinColumns(): Unit =
    val found = HotJoinDetector.detect(ctx(workload((joinShape, 100L, 60 * second, 100L, 100L, 1.0))))
    assertEquals(1, found.size)
    assertTrue(found.head.isInstanceOf[Informational])
    assertTrue(found.head.summary.contains("orders.customer_id"), found.head.summary)
    assertTrue(found.head.summary.contains("customers.id"), found.head.summary)

  @Test def aBadEstimateOnNeverAnalyzedTableRecommendsAnalyze(): Unit =
    val found = CardinalityErrorDetector.detect(ctx(workload((ordersByCustomer, 100L, second, 100L, 100L, 250.0))))
    assertEquals(Vector(RunAnalyze(orders, "orders", "it was never analyzed", Vector(ordersByCustomer.id))), found)

  @Test def aBadEstimateOnStaleStatisticsRecommendsAnalyze(): Unit =
    val table = TableStatistics(EntityId(orders), 1000L, 64.0, 1L, 0L, 1000L, true, Map.empty, Map.empty, 1L)
    val stale = design(stats = Some(EntityStatistics(table, 600L)))
    val found = CardinalityErrorDetector.detect(ctx(workload((ordersByCustomer, 100L, second, 100L, 100L, 250.0)), stale))
    assertTrue(found.head.isInstanceOf[RunAnalyze])
    assertTrue(found.head.summary.contains("60%"), found.head.summary)

  @Test def aBadEstimateOnFreshStatisticsIsOnlyInformation(): Unit =
    val table = TableStatistics(EntityId(orders), 1000L, 64.0, 1L, 0L, 1000L, true, Map.empty, Map.empty, 1L)
    val fresh = design(stats = Some(EntityStatistics(table, 5L)))
    val found = CardinalityErrorDetector.detect(ctx(workload((ordersByCustomer, 100L, second, 100L, 100L, 250.0)), fresh))
    assertTrue(found.head.isInstanceOf[Informational])
    assertEquals(None, found.head.sql)

  @Test def smallErrorsAreIgnored(): Unit =
    assertTrue(CardinalityErrorDetector.detect(ctx(workload((ordersByCustomer, 100L, second, 100L, 100L, 3.0)))).isEmpty)
