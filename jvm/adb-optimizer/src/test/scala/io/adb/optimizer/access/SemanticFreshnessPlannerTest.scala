package io.adb.optimizer.access

import io.adb.catalog.{InMemoryCatalog, IndexInfo, IndexKeyColumn}
import io.adb.consistency.*
import io.adb.logical.*
import io.adb.model.*
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Which access paths a session's consistency level allows. */
final class SemanticFreshnessPlannerTest:
  /** account(id, region). */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("account", Vector(("id", DataType.Int64, false), ("region", DataType.Int64, false)), "id")
    c
  /** An index on region. */
  private val regionIndex =
    IndexInfo(IndexId(1), "by_region", catalog.entity("account").get.id, Vector(IndexKeyColumn(FieldId(2), false)), Vector.empty, 1, 1, 10)

  /** The candidate paths for `WHERE region = 3`: a sequential scan and an index scan. */
  private def candidates: Vector[AccessPath] =
    val select = new Binder(catalog).bind(new SqlParser().parse("SELECT a.id FROM account a WHERE a.region = 3")).asInstanceOf[BoundSelect]
    AccessPathEnumerator.candidates(select.base, select.predicate.toVector.flatMap(TypedExpr.conjuncts), Vector(regionIndex), None)

  /** The index's projection, at `at`. */
  private def projection(at: Long, rate: Double = 0.0, state: ProjectionState = ProjectionState.Ready) =
    ProjectionInfo(1001, "index:by_region", ProjectionKind.SecondaryIndex, ProjectionMode.Asynchronous, None, 1, state,
      ProjectionFrontier(at, at), 20, 20 - at, 0, 0, rate)

  /** A read-your-writes context with the given token at head 20. */
  private def ryw(token: Long) = SemanticReadContext(ReadConsistency.ReadYourWrites, SessionConsistencyToken(token), 20, 1000)

  /** A caught-up index keeps its path; the sequential scan is never touched. */
  @Test def freshIndexIsUsable(): Unit =
    val annotated = SemanticFreshnessPlanner.annotate(candidates, Vector(projection(20)), ryw(15))
    assertEquals(2, annotated.size)
    assertTrue(annotated.forall(_.resolution == FreshnessResolution.Direct))
    assertEquals(2, SemanticFreshnessPlanner.usable(annotated).size)

  /** A stale index is dropped, leaving the sequential scan. */
  @Test def staleIndexIsDropped(): Unit =
    val annotated = SemanticFreshnessPlanner.annotate(candidates, Vector(projection(5, 1.0)), ryw(15))
    val usable = SemanticFreshnessPlanner.usable(annotated)
    assertEquals(1, usable.size)
    assertTrue(usable.head.path.isInstanceOf[AccessPath.SeqScan])

  /** A slightly stale index is kept with a wait that the executor must perform first. */
  @Test def slightlyStaleIndexWaits(): Unit =
    val annotated = SemanticFreshnessPlanner.annotate(candidates, Vector(projection(10, 10.0)), ryw(15))
    val scan = SemanticFreshnessPlanner.usable(annotated).find(_.path.isInstanceOf[AccessPath.IndexScan]).get
    assertEquals(Some((1001L, 15L)), SemanticFreshnessPlanner.waits(scan))

  /** Without a report the index behaves as in 2.4.0: synchronous and fresh. */
  @Test def unreportedIndexesAreTreatedAsFresh(): Unit =
    val annotated = SemanticFreshnessPlanner.annotate(candidates, Vector.empty, ryw(15))
    assertTrue(annotated.forall(_.resolution == FreshnessResolution.Direct))
    assertEquals(None, SemanticFreshnessPlanner.waits(annotated.head))

  /** An index being built is never read. */
  @Test def buildingIndexIsDropped(): Unit =
    val annotated = SemanticFreshnessPlanner.annotate(candidates, Vector(projection(20, state = ProjectionState.Building)), ryw(0))
    assertEquals(1, SemanticFreshnessPlanner.usable(annotated).size)

  /** The per-index decisions used by the gateway agree with the per-path ones. */
  @Test def decidesPerIndex(): Unit =
    val stale = SemanticFreshnessPlanner.resolveIndexes(Vector(regionIndex), Vector(projection(5, 1.0)), ryw(15))
    assertFalse(SemanticFreshnessPlanner.isUsable(stale.head.resolution))
    val waiting = SemanticFreshnessPlanner.resolveIndexes(Vector(regionIndex), Vector(projection(10, 10.0)), ryw(15))
    assertEquals(FreshnessResolution.WaitUntil(15, Some(500)), waiting.head.resolution)
    assertTrue(SemanticFreshnessPlanner.isUsable(waiting.head.resolution))
    assertEquals(FreshnessResolution.Direct, SemanticFreshnessPlanner.resolveIndexes(Vector(regionIndex), Vector.empty, ryw(15)).head.resolution)
