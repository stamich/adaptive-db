package io.adb.optimizer.access

import io.adb.catalog.{InMemoryCatalog, IndexInfo, IndexKeyColumn}
import io.adb.logical.*
import io.adb.model.*
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Matching filter conjuncts against indexes, and the orders access paths produce. */
final class IndexPredicateMatcherTest:
  /** account(id, region, balance DOUBLE, owner). */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("account", Vector(("id", DataType.Int64, false), ("region", DataType.Int64, false),
      ("balance", DataType.Float64, false), ("owner", DataType.StringType, true)), "id")
    c
  /** Field ids. */
  private val (id, region, balance, owner) = (FieldId(1), FieldId(2), FieldId(3), FieldId(4))

  /** An index on `columns` (field, descending) covering `include`. */
  private def index(columns: Vector[(FieldId, Boolean)], include: Vector[FieldId] = Vector.empty): IndexInfo =
    IndexInfo(IndexId(1), "idx", catalog.entity("account").get.id, columns.map(IndexKeyColumn.apply), include, 1, 1, 10)

  /** The relation and WHERE conjuncts of `sql`. */
  private def bound(sql: String): (BoundRelation, Vector[TypedExpr], BoundSelect) =
    val select = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
    (select.base, select.predicate.toVector.flatMap(TypedExpr.conjuncts), select)

  /** Equality prefix, then the tightest range on the next column; the rest is residual. */
  @Test def matchesPrefixAndRange(): Unit =
    val (relation, conjuncts, _) = bound(
      "SELECT a.id FROM account a WHERE a.region = 3 AND a.balance > 10.0 AND a.balance >= 20.0 AND 50.0 > a.balance AND a.owner = 'x'")
    val matched = IndexPredicateMatcher.matchIndex(relation, index(Vector(region -> false, balance -> false)), conjuncts)
    assertEquals(Vector(DbValue.Int64Value(3)), matched.equalities.map(_._3))
    assertEquals(Some(RangeEnd(DbValue.Float64Value(20.0), true)), matched.lower.map(_._1))
    assertEquals(Some(RangeEnd(DbValue.Float64Value(50.0), false)), matched.upper.map(_._1))
    assertEquals(2, matched.residual.size, "balance > 10 is implied but kept; owner = 'x' is not indexed")
    assertTrue(matched.isRestricting)
    assertEquals(Some(IndexKeyColumn(balance, false)), matched.rangeColumn)

  /** A gap in the key stops the prefix; NULL, OR and <> are never index bounds. */
  @Test def stopsAtGapsAndIgnoresUnusableConjuncts(): Unit =
    val (relation, conjuncts, _) = bound("SELECT a.id FROM account a WHERE a.balance = 5.0 AND a.owner <> 'x' AND (a.region = 1 OR a.region = 2)")
    val matched = IndexPredicateMatcher.matchIndex(relation, index(Vector(region -> false, balance -> false)), conjuncts)
    assertFalse(matched.isRestricting, "region has no equality, so balance cannot be used")
    assertEquals(3, matched.residual.size)
    val (r2, c2, _) = bound("SELECT a.id FROM account a WHERE a.balance = 5.5")
    assertEquals(1, IndexPredicateMatcher.matchIndex(r2, index(Vector(balance -> false)), c2).equalities.size)
    assertEquals(None, IndexPredicateMatcher.convert(DbValue.NullValue, DataType.Int64))
    assertEquals(None, IndexPredicateMatcher.convert(DbValue.Float64Value(1.5), DataType.Int64))
    assertEquals(Some(DbValue.Float64Value(7.0)), IndexPredicateMatcher.convert(DbValue.Int64Value(7), DataType.Float64))

  /** Index scans produce index order after the equality prefix, then primary-key order; a
    * backward scan the reverse; sequential scans primary-key order.
    */
  @Test def accessPathsKnowTheirOrder(): Unit =
    val (relation, conjuncts, select) = bound("SELECT a.id, a.balance, a.owner FROM account a WHERE a.region = 3")
    val slot = (name: String) => relation.columns.find(_.name.endsWith(name)).get.slot
    val matched = IndexPredicateMatcher.matchIndex(relation, index(Vector(region -> false, balance -> true), Vector(owner)), conjuncts)
    val forward = AccessPath.IndexScan(relation, matched, backward = false, covering = false)
    assertEquals(PhysicalProperties(Vector(SortOrder(slot("balance"), true), SortOrder(slot("id"), false))), forward.properties)
    val backward = forward.copy(backward = true)
    assertEquals(PhysicalProperties(Vector(SortOrder(slot("balance"), false), SortOrder(slot("id"), true))), backward.properties)
    assertTrue(forward.properties.satisfies(Vector(SortOrder(slot("balance"), true))))
    assertTrue(forward.properties.satisfies(Vector(SortOrder(slot("region"), false), SortOrder(slot("balance"), true)), forward.constantSlots))
    assertFalse(forward.properties.satisfies(Vector(SortOrder(slot("balance"), false))))
    assertFalse(forward.properties.satisfies(Vector(SortOrder(slot("owner"), false))))
    assertEquals(PhysicalProperties(Vector(SortOrder(slot("id"), false))), AccessPath.SeqScan(relation, conjuncts).properties)
    assertTrue(AccessPath.covers(relation, index(Vector(region -> false, balance -> false), Vector(owner, id))))
    assertFalse(AccessPath.covers(relation, index(Vector(region -> false))))
    assertEquals(select.base, relation)
