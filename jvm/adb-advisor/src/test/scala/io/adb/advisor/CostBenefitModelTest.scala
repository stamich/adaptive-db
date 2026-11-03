package io.adb.advisor

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the scoring and of the recommendation texts. */
final class CostBenefitModelTest:
  private val second = 1_000_000_000L
  private val model = CostBenefitModel(horizonNanos = 3600L * second, buildNanosPerRow = 1000.0, writePenalty = 0.0, minRatio = 2.0, minExecutions = 10L)

  /** A shape that ran `n` times in total `totalNanos`, reading `read` rows and returning `returned`. */
  private def delta(n: Long, totalNanos: Long, read: Long, returned: Long) = ShapeDelta(1L, n, 0L, totalNanos, returned, read, 1.0)

  @Test def benefitIsTheWastedShareOfTimeScaledToTheHorizon(): Unit =
    // 100 executions, 10 s in total over a 60 s span; 99 % of the rows read were not needed.
    val d = delta(100, 10 * second, 100_000, 1_000)
    val expected = 10.0 * second * 0.99 * 60.0
    assertEquals(expected, model.indexBenefit(d, 60 * second), expected * 1e-9)

  @Test def thinOrUnselectiveEvidenceHasNoBenefit(): Unit =
    assertEquals(0.0, model.indexBenefit(delta(5, second, 1000, 1), 60 * second), "fewer than minExecutions")
    assertEquals(0.0, model.indexBenefit(delta(100, second, 1000, 1000), 60 * second), "every row read was returned")
    assertEquals(0.0, model.indexBenefit(delta(100, second, 0, 0), 60 * second), "nothing was scanned")
    assertEquals(0.0, model.indexBenefit(delta(100, second, 1000, 1), 0L), "no observed span")

  @Test def costGrowsWithTheTableAndTheWritePenalty(): Unit =
    assertEquals(1_000_000.0, model.indexCost(1000), 1e-6)
    assertEquals(1_500_000.0, model.copy(writePenalty = 0.5).indexCost(1000), 1e-6)
    assertEquals(1000.0, model.indexCost(0), 1e-6, "an empty table still costs one row")

  @Test def theRatioGatesTheRecommendation(): Unit =
    assertTrue(model.worthIt(Score(3.0, 1.0)))
    assertFalse(model.worthIt(Score(1.9, 1.0)))
    assertEquals(0.0, Score(0.0, 5.0).ratio)
    assertEquals(Double.PositiveInfinity, Score(5.0, 0.0).ratio)

  @Test def recommendationsDescribeThemselves(): Unit =
    val index = CreateIndex(7L, "orders", Vector(2L, 3L), Vector("customer_id", "amount"), Some(Score(6.0, 2.0)), Vector(11L))
    assertEquals("CREATE INDEX adv_orders_customer_id_amount ON orders (customer_id, amount)", index.sql.get)
    assertEquals("index:7:2,3", index.key)
    assertTrue(index.summary.contains("benefit/cost 3.0"), index.summary)
    val analyze = RunAnalyze(7L, "orders", "statistics are stale", Vector.empty)
    assertEquals(Some("ANALYZE orders"), analyze.sql)
    assertEquals("analyze:7", analyze.key)
    val info = Informational("hot-join", "join:1", "orders joins customers on every request", Vector(1L))
    assertEquals(None, info.sql)
    assertEquals("hot-join: orders joins customers on every request", info.summary)
