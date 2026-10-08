package io.adb.gateway

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.OptimizerConfig
import io.adb.physical.PhysicalPlan
import io.adb.sql.SqlParser
import io.adb.statistics.*
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The gateway's planning pipeline (cost and rule mode), EXPLAIN sections, the engine
  * statistics cache and the feedback log, without the native library.
  */
final class CostBasedGatewayTest:
  /** customer(id, name) <- orders(id, customer_id REFERENCES customer, amount). */
  private val catalog =
    val c = new InMemoryCatalog
    val customer = c.createEntity("customer", Vector(("id", DataType.Int64, false), ("name", DataType.StringType, false)), "id")
    c.createEntity("orders", Vector(("id", DataType.Int64, false), ("customer_id", DataType.Int64, false), ("amount", DataType.Int64, false)), "id",
      Map("customer_id" -> ForeignKeyRef(customer.id, customer.primaryKey)))
    c
  /** 100 customers and 1,000 orders, analyzed; orders changed by 30% since. */
  private val provider = StatisticsProvider.of(
    EntityStatistics(TableStatistics(catalog.entity("customer").get.id, 100, 30, 7, 0, 100, true, Map.empty), 0),
    EntityStatistics(TableStatistics(catalog.entity("orders").get.id, 1000, 30, 7, 0, 1000, true, Map.empty), 300)
  )
  /** The benchmark's workload A. */
  private val Query = "SELECT c.name, o.amount FROM customer c JOIN orders o ON c.id = o.customer_id ORDER BY o.amount DESC LIMIT 10"

  /** Plans `sql` in `mode`. */
  private def plans(sql: String, mode: OptimizerMode): AdaptiveDatabase.Plans =
    val select = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    AdaptiveDatabase.plan(select, catalog, provider, OptimizerConfig.Default, mode)

  /** Cost mode explains join order, strategies, statistics and warnings; every node has an estimate. */
  @Test def costModeExplain(): Unit =
    val p = plans(Query, OptimizerMode.Cost)
    val sections = AdaptiveDatabase.explain(p)
    val text = sections.mkString("\n\n")
    assertTrue(sections.exists(_.startsWith("Physical (optimizer=cost):\n[0] ")), text)
    assertTrue(text.contains("join order: (c JOIN o)") || text.contains("join order: (o JOIN c)"), text)
    assertTrue(text.contains("- orders: 1000 rows analyzed at ts=7, 300 modifications since (stale)"), text)
    assertTrue(text.contains("Warnings:\n- statistics of orders are stale"), text)
    assertEquals(PhysicalPlan.preorder(p.physical.plan).indices.toSet, p.physical.estimates.keySet)
    val join = PhysicalPlan.preorder(p.physical.plan).indexWhere(_.isInstanceOf[PhysicalPlan.HashJoin])
    assertEquals(1000.0, p.physical.estimates(join).rows, 1e-9, "foreign key: one customer per order")

  /** Rule mode keeps the SQL order and the right build side but still shows estimates. */
  @Test def ruleModeExplain(): Unit =
    val p = plans("SELECT c.name FROM orders o JOIN customer c ON c.id = o.customer_id", OptimizerMode.Rule)
    val text = AdaptiveDatabase.explain(p).mkString("\n\n")
    assertTrue(text.contains("Physical (optimizer=rule)"), text)
    assertFalse(text.contains("join order"), text)
    assertTrue(text.contains("builds the right input"), text)
    assertTrue(text.contains("est. rows="), text)

  /** Documents are decoded once until invalidated; the modification count is always current. */
  @Test def engineStatisticsAreCached(): Unit =
    var documents = 0
    var modifications = 0L
    val json =
      """{"format_version":1,"entity_id":1,"row_count":5,"avg_row_bytes":8.0,"analyzed_at_ts":1,
        |"modifications_at_analyze":0,"sampled_rows":5,"exact":true,"columns":[]}""".stripMargin
    val engine = new EngineStatisticsProvider(id => { documents += 1; Option.when(id == 1)(json) }, _ => modifications)
    assertEquals(None, engine.statistics(EntityId(2)))
    assertEquals(5L, engine.statistics(EntityId(1)).get.table.rowCount)
    modifications = 3
    assertEquals(3L, engine.statistics(EntityId(1)).get.modificationsSinceAnalyze)
    assertEquals(2, documents)
    engine.statistics(EntityId(2))
    assertEquals(2, documents, "a missing document is cached too")
    engine.invalidate(EntityId(1))
    engine.statistics(EntityId(1))
    assertEquals(3, documents)

  /** The feedback log writes one line per query without SQL literals and rotates at its bound. */
  @Test def feedbackLogRotates(): Unit =
    val dir = Files.createTempDirectory("adb-feedback")
    val log = new PlannerFeedbackLog(dir.resolve("planner-feedback.jsonl"), maxBytes = 1024)
    val comparisons = Vector(ProfileRenderer.Comparison(0, "top_k", 10, 12), ProfileRenderer.Comparison(1, "hash_join", 100, 50))
    log.record("SELECT secret FROM t WHERE name = 'Alice'", "cost", comparisons)
    val first = Files.readString(log.path, StandardCharsets.UTF_8)
    assertTrue(first.startsWith("{\"ts_ms\":"), first)
    assertTrue(first.contains("\"node_id\":1,\"operator\":\"hash_join\",\"estimated_rows\":100.0,\"actual_rows\":50,\"q_error\":2.000"), first)
    assertFalse(first.contains("Alice"), "literals never reach the log")
    log.record("SELECT 1", "rule", Vector.empty)
    assertEquals(first, Files.readString(log.path, StandardCharsets.UTF_8), "queries without estimates are not logged")
    for _ <- 0 until 10 do log.record("SELECT x", "cost", comparisons)
    assertTrue(Files.exists(dir.resolve("planner-feedback.jsonl.1")))
    assertTrue(Files.size(log.path) <= 1024)
