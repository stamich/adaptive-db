package io.adb.optimizer.cardinality

import io.adb.catalog.InMemoryCatalog
import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*
import io.adb.optimizer.RuleOptimizer
import io.adb.sql.SqlParser
import io.adb.statistics.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Cardinality estimation rules, with and without statistics. */
final class CardinalityEstimatorTest:
  /** customer(id, name, country) and orders(id, customer_id -> customer, amount). */
  private val catalog =
    val c = new InMemoryCatalog
    val customer = c.createEntity("customer", Vector(("id", DataType.Int64, false), ("name", DataType.StringType, false), ("country", DataType.StringType, true)), "id")
    c.createEntity("orders", Vector(("id", DataType.Int64, false), ("customer_id", DataType.Int64, true), ("amount", DataType.Int64, false)), "id",
      Map("customer_id" -> ForeignKeyRef(customer.id, customer.primaryKey)))
    c.createEntity("plain", Vector(("id", DataType.Int64, false), ("customer_id", DataType.Int64, false)), "id")
    c
  /** Entity ids. */
  private val customer = catalog.entity("customer").get.id
  private val orders = catalog.entity("orders").get.id

  /** Int64 value shorthand. */
  private def i(v: Long): DbValue = DbValue.Int64Value(v)
  /** String value shorthand. */
  private def s(v: String): DbValue = DbValue.StringValue(v)

  /** 100 customers: unique ids 1..100 in 4 buckets, 10% NULL countries, PL 60 / DE 30. */
  private val customerStats = TableStatistics(customer, 100, 30, 5, 0, 100, true, Map(
    FieldId(1) -> ColumnStatistics(FieldId(1), 0, 100, true, Some(i(1)), Some(i(100)), 8,
      (0 until 4).map(b => HistogramBucket(i(b * 25 + 1), i(b * 25 + 25), 25, 25)).toVector, Vector.empty),
    FieldId(3) -> ColumnStatistics(FieldId(3), 10, 2, true, Some(s("DE")), Some(s("PL")), 2, Vector.empty,
      Vector(MostCommonValue(s("PL"), 60), MostCommonValue(s("DE"), 30)))
  ))
  /** 1,000 orders: 5% NULL customer ids over 100 customers; amounts 0..999 without histogram. */
  private val orderStats = TableStatistics(orders, 1000, 40, 5, 0, 1000, true, Map(
    FieldId(2) -> ColumnStatistics(FieldId(2), 50, 100, true, Some(i(1)), Some(i(100)), 8, Vector.empty, Vector.empty),
    FieldId(3) -> ColumnStatistics(FieldId(3), 0, 1000, true, Some(i(0)), Some(i(999)), 8, Vector.empty, Vector.empty)
  ))
  /** Both entities analyzed and unchanged. */
  private val fresh = StatisticsProvider.of(EntityStatistics(customerStats, 0), EntityStatistics(orderStats, 0))

  /** The optimized logical plan of `sql`. */
  private def plan(sql: String): LogicalPlan =
    val bound = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    new RuleOptimizer().optimize(LogicalPlanner.plan(bound))

  /** The first operator of `plan` matching `pick`, searching depth first. */
  private def find(plan: LogicalPlan)(pick: PartialFunction[LogicalPlan, LogicalPlan]): LogicalPlan =
    pick.lift(plan).getOrElse(plan.children.iterator.map(child => scala.util.Try(find(child)(pick))).collectFirst { case scala.util.Success(p) => p }
      .getOrElse(throw new NoSuchElementException("no matching operator")))

  /** Rows estimated for the first filter (or the root) of `sql`. */
  private def filterRows(sql: String, provider: StatisticsProvider = fresh): Double =
    val p = plan(sql)
    new CardinalityEstimator(provider, catalog).estimate(find(p) { case f: Filter => f }).rows

  /** Without statistics: 1,000 rows at confidence 0.1, default selectivities. */
  @Test def defaultsWithoutStatistics(): Unit =
    val estimator = new CardinalityEstimator(StatisticsProvider.Empty, catalog)
    val scan = estimator.estimate(find(plan("SELECT name FROM customer")) { case t: TableScan => t })
    assertEquals(Estimate(1000, 0.1, EstimateSource.Default), scan)
    assertEquals(100.0, filterRows("SELECT name FROM customer WHERE country = 'PL'", StatisticsProvider.Empty), 1e-9)
    assertEquals(1000.0 / 3, filterRows("SELECT name FROM customer WHERE name > 'M'", StatisticsProvider.Empty), 1e-9)

  /** Primary-key equality is one row; the point-lookup rewrite keeps it at most one. */
  @Test def primaryKeyEquality(): Unit =
    val estimator = new CardinalityEstimator(fresh, catalog)
    assertEquals(1.0, estimator.estimate(plan("SELECT name FROM customer WHERE id = 7")).rows)
    assertEquals(100 * (0.02 - 0.0001), filterRows("SELECT name FROM customer WHERE id = 7 OR id = 8"), 1e-9)

  /** Common values use their sampled frequency; others share the remaining rows. */
  @Test def mostCommonValues(): Unit =
    assertEquals(60.0, filterRows("SELECT name FROM customer WHERE country = 'PL'"), 1e-9)
    assertEquals(30.0, filterRows("SELECT name FROM customer WHERE country = 'DE'"), 1e-9)
    assertEquals(1.0, filterRows("SELECT name FROM customer WHERE country = 'ZZ'"), 1e-9, "outside [min, max]: clamped to one row")
    assertEquals(30.0, filterRows("SELECT name FROM customer WHERE country <> 'PL'"), 1e-9, "non-NULL rows minus PL")

  /** Ranges use the histogram, or min/max interpolation without one. */
  @Test def ranges(): Unit =
    assertEquals(50.0, filterRows("SELECT name FROM customer WHERE id <= 50"), 1.0)
    assertEquals(25.0, filterRows("SELECT name FROM customer WHERE id > 75"), 1.0)
    assertEquals(1.0, filterRows("SELECT name FROM customer WHERE id > 1000"), 1e-9)
    assertEquals(100.0, filterRows("SELECT o.id FROM orders o WHERE o.amount < 100"), 1.0)
    assertEquals(1000.0, filterRows("SELECT o.id FROM orders o WHERE o.amount >= 0"), 1e-9)

  /** AND multiplies, OR adds minus the overlap, NOT complements. */
  @Test def connectives(): Unit =
    assertEquals(60.0 * 0.5, filterRows("SELECT name FROM customer WHERE country = 'PL' AND id <= 50"), 1.0)
    assertEquals(100 * (0.6 + 0.3 - 0.18), filterRows("SELECT name FROM customer WHERE country = 'PL' OR country = 'DE'"), 1e-6)
    assertEquals(40.0, filterRows("SELECT name FROM customer WHERE NOT (country = 'PL')"), 1e-9)

  /** A declared foreign key bounds the join by the child side; filtering the parent scales it. */
  @Test def foreignKeyJoin(): Unit =
    val estimator = new CardinalityEstimator(fresh, catalog)
    val joined = plan("SELECT c.name, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id")
    val join = estimator.estimate(find(joined) { case j: Join => j })
    assertEquals(950.0, join.rows, 1e-9, "1,000 orders minus 5% NULL foreign keys")
    assertEquals(0.95, join.confidence, 1e-9)
    val filtered = plan("SELECT c.name, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id WHERE c.country = 'PL'")
    assertEquals(950.0 * 0.6, new CardinalityEstimator(fresh, catalog).estimate(find(filtered) { case j: Join => j }).rows, 1e-6)

  /** Without a hint the join uses distinct counts; LEFT JOIN keeps every left row. */
  @Test def distinctCountJoinAndLeftJoin(): Unit =
    val estimator = new CardinalityEstimator(fresh, catalog)
    // plain is not analyzed: 1,000 default rows, customer_id with 200 default distinct values.
    val join = estimator.estimate(find(plan("SELECT c.name FROM customer c JOIN plain p ON p.customer_id = c.id")) { case j: Join => j })
    assertEquals(100.0 * 1000 / 200, join.rows, 1e-9)
    assertEquals(EstimateSource.Default, join.source)
    val left = estimator.estimate(find(plan("SELECT c.name FROM customer c LEFT JOIN orders o ON o.customer_id = c.id AND o.amount > 990")) { case j: Join => j })
    assertTrue(left.rows >= 100.0, left.toString)
    val cross = estimator.estimate(find(plan("SELECT c.name FROM customer c CROSS JOIN orders o")) { case j: Join => j })
    assertEquals(100.0 * 1000, cross.rows, 1e-9)

  /** Groups are the product of distinct counts, capped by the input; no GROUP BY is one row. */
  @Test def aggregates(): Unit =
    val estimator = new CardinalityEstimator(fresh, catalog)
    assertEquals(2.0, estimator.estimate(find(plan("SELECT country, COUNT(*) FROM customer GROUP BY country")) { case a: Aggregate => a }).rows)
    assertEquals(100.0, estimator.estimate(find(plan("SELECT id, country, COUNT(*) FROM customer GROUP BY id, country")) { case a: Aggregate => a }).rows)
    assertEquals(1.0, estimator.estimate(find(plan("SELECT COUNT(*) FROM orders")) { case a: Aggregate => a }).rows)
    assertEquals(10.0, estimator.estimate(plan("SELECT name FROM customer ORDER BY name LIMIT 10")).rows)

  /** More than 20% changed rows makes statistics stale: same rows, half the confidence. */
  @Test def staleStatistics(): Unit =
    val stale = StatisticsProvider.of(EntityStatistics(customerStats, 21), EntityStatistics(orderStats, 0))
    val estimator = new CardinalityEstimator(stale, catalog)
    val p = plan("SELECT name FROM customer")
    assertEquals(Estimate(100, 0.5, EstimateSource.Stale), estimator.estimate(find(p) { case t: TableScan => t }))
    val relation = find(p) { case t: TableScan => t }.asInstanceOf[TableScan].relation
    assertTrue(estimator.status(relation).isInstanceOf[StatisticsStatus.Stale])

  /** Statistics are fetched once per entity and estimates memoized per node. */
  @Test def fetchesStatisticsOnce(): Unit =
    var calls = 0
    val counting: StatisticsProvider = entity => { calls += 1; fresh.statistics(entity) }
    val estimator = new CardinalityEstimator(counting, catalog)
    val p = plan("SELECT c.name FROM customer c JOIN orders o ON o.customer_id = c.id WHERE c.country = 'PL' AND o.amount > 5")
    estimator.estimate(p)
    estimator.estimate(p)
    assertEquals(2, calls)

  /** Bounded construction never yields NaN, infinite or negative estimates. */
  @Test def boundedEstimates(): Unit =
    assertEquals(Estimate.MaxRows, Estimate.bounded(Double.PositiveInfinity, 2, EstimateSource.Default).rows)
    assertEquals(0.0, Estimate.bounded(-5, 0.5, EstimateSource.Default).rows)
    assertEquals(0.01, Estimate.bounded(5, Double.NaN, EstimateSource.Default).confidence)
    assertEquals(EstimateSource.Default, EstimateSource.weakest(EstimateSource.Statistics, EstimateSource.Default))

  /** EXPLAIN's basis names the rule behind each comparison and join. */
  @Test def selectivityBasis(): Unit =
    val estimator = new CardinalityEstimator(fresh, catalog)
    def basis(sql: String): Vector[String] =
      estimator.basis(find(plan(sql)) { case f: Filter => f }.asInstanceOf[Filter].predicate)
    assertEquals(Vector("mcv"), basis("SELECT name FROM customer WHERE country = 'PL'"))
    assertEquals(Vector("min/max"), basis("SELECT name FROM customer WHERE country = 'ZZ'"))
    assertEquals(Vector("histogram", "default"), basis("SELECT name FROM customer WHERE id < 5 OR name = 'x'"))
    assertEquals(Vector("min/max"), basis("SELECT o.id FROM orders o WHERE o.amount > 3"))
    assertEquals(Vector("primary key"), basis("SELECT name FROM customer WHERE id = 1 OR id = 2"))
    val fk = find(plan("SELECT c.name FROM customer c JOIN orders o ON o.customer_id = c.id")) { case j: Join => j }
    assertEquals("foreign key", estimator.joinBasis(fk.asInstanceOf[Join]))
    val plain = find(plan("SELECT c.name FROM customer c JOIN plain p ON p.customer_id = c.id")) { case j: Join => j }
    assertEquals("ndv", estimator.joinBasis(plain.asInstanceOf[Join]))
    val cross = find(plan("SELECT c.name FROM customer c CROSS JOIN plain p")) { case j: Join => j }
    assertEquals("cross product", estimator.joinBasis(cross.asInstanceOf[Join]))

