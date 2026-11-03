package io.adb.gateway

import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.consistency.*
import io.adb.model.*
import io.adb.optimizer.access.SemanticFreshnessPlanner
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The consistency scenarios of Milestone 2.5.0 played against a scripted engine: sessions, the
  * levels, waits and fallbacks, with no native library.
  */
final class FreshnessAcceptanceTest:
  /** A scripted engine: a head, and one asynchronous index projection that lags and catches up. */
  private final class Engine(var head: Long, var projectionAt: Long, var rate: Double) extends ConsistencyBackend:
    /** Waits requested, as (projection, commit). */
    var waits: Vector[(Long, Long)] = Vector.empty
    /** Whether a wait succeeds (the projection catches up). */
    var catchesUp: Boolean = true
    /** @inheritdoc */
    def latestCommitSeq(): Long = head
    /** @inheritdoc */
    def projections(): Vector[ProjectionInfo] = Vector(ProjectionInfo(1001, "index:by_region", ProjectionKind.SecondaryIndex,
      ProjectionMode.Asynchronous, Some(1), 1, ProjectionState.Ready, ProjectionFrontier(projectionAt, projectionAt),
      head, head - projectionAt, 0, 0, rate))
    /** @inheritdoc */
    def awaitProjection(id: Long, commitSeq: Long, timeoutMillis: Long): Boolean =
      waits :+= ((id, commitSeq))
      if catchesUp then projectionAt = math.max(projectionAt, commitSeq)
      catchesUp

  /** The index behind the projection. */
  private val index = IndexInfo(IndexId(1), "by_region", EntityId(1), Vector(IndexKeyColumn(FieldId(2), false)), Vector.empty, 1, 1, 10)

  /** A session over `engine`. */
  private def session(engine: Engine) = new AdaptiveSession(null, engine)

  /** What the session's next read does with the index. */
  private def decision(engine: Engine, s: AdaptiveSession): FreshnessResolution =
    SemanticFreshnessPlanner.resolveIndexes(Vector(index), engine.projections(), s.readContext()).head.resolution

  /** Read-your-writes, fresh: the index already contains the session's write. */
  @Test def readYourWritesWithAFreshIndex(): Unit =
    val engine = new Engine(head = 20, projectionAt = 20, rate = 10)
    val s = session(engine)
    s.observe(18L)
    assertEquals(FreshnessResolution.Direct, decision(engine, s))

  /** Read-your-writes, stale: the index lags behind the session's write and cannot be used. */
  @Test def readYourWritesWithAStaleIndex(): Unit =
    val engine = new Engine(head = 20, projectionAt = 10, rate = 1)
    val s = session(engine)
    s.observe(18L)
    assertTrue(decision(engine, s).isInstanceOf[FreshnessResolution.Fallback])
    // A faster projection is waited for instead.
    engine.rate = 100
    assertEquals(FreshnessResolution.WaitUntil(18, Some(80)), decision(engine, s))

  /** Session A wrote and must see its write; session B wrote nothing and may read the lagging index. */
  @Test def sessionsSeeTheirOwnWritesOnly(): Unit =
    val engine = new Engine(head = 20, projectionAt = 12, rate = 1)
    val (a, b) = (session(engine), session(engine))
    a.observe(18L)
    assertTrue(decision(engine, a).isInstanceOf[FreshnessResolution.Fallback])
    assertEquals(FreshnessResolution.Direct, decision(engine, b))

  /** Strong reads need the head regardless of the session's own writes. */
  @Test def strongLatestNeedsTheHead(): Unit =
    val engine = new Engine(head = 20, projectionAt = 19, rate = 0)
    val s = session(engine)
    s.consistency = ReadConsistency.StrongLatest
    assertTrue(decision(engine, s).isInstanceOf[FreshnessResolution.Fallback])
    engine.projectionAt = 20
    assertEquals(FreshnessResolution.Direct, decision(engine, s))

  /** Eventual reads take whatever the index holds. */
  @Test def eventualNeverWaits(): Unit =
    val engine = new Engine(head = 20, projectionAt = 3, rate = 0)
    val s = session(engine)
    s.observe(19L)
    s.consistency = ReadConsistency.Eventual
    assertEquals(FreshnessResolution.AllowStale(17), decision(engine, s))

  /** Bounded staleness accepts a small lag and refuses a large one. */
  @Test def boundedStaleness(): Unit =
    val engine = new Engine(head = 20, projectionAt = 17, rate = 0)
    val s = session(engine)
    s.consistency = ReadConsistency.BoundedStaleness(StalenessBound.Commits(5))
    assertEquals(FreshnessResolution.AllowStale(3), decision(engine, s))
    engine.projectionAt = 10
    assertTrue(decision(engine, s).isInstanceOf[FreshnessResolution.Fallback])

  /** A wait that times out leaves the index out of the plan; one that succeeds keeps it. */
  @Test def waitTimeoutFallsBack(): Unit =
    val engine = new Engine(head = 20, projectionAt = 10, rate = 100)
    val s = session(engine)
    s.observe(18L)
    val decisions = SemanticFreshnessPlanner.resolveIndexes(Vector(index), engine.projections(), s.readContext())
    val waits = SemanticFreshnessPlanner.waitsFor(decisions, Set(index.id))
    assertEquals(Vector((1001L, 18L)), waits.map(w => (w._2, w._3)))
    assertTrue(engine.awaitProjection(waits.head._2, waits.head._3, 100), "the projection catches up")
    assertEquals(FreshnessResolution.Direct, decision(engine, s))
    // An index the plan does not read is never waited for.
    assertEquals(Vector.empty, SemanticFreshnessPlanner.waitsFor(decisions, Set.empty))

  /** A token written by another database is refused rather than answered from old data. */
  @Test def tokenFromTheFutureIsInfeasible(): Unit =
    val engine = new Engine(head = 5, projectionAt = 5, rate = 0)
    val s = session(engine)
    s.observe(50L)
    assertTrue(s.readContext().tokenIsAhead)
    assertTrue(decision(engine, s).isInstanceOf[FreshnessResolution.Infeasible])
    s.consistency = ReadConsistency.Eventual
    assertEquals(FreshnessResolution.Direct, decision(engine, s))

  /** Commit sequence numbers order DML and DDL together: 10, 11 (DDL), 12. */
  @Test def dmlAndDdlShareOneOrder(): Unit =
    val engine = new Engine(head = 12, projectionAt = 12, rate = 0)
    val s = session(engine)
    s.observe(10L) // INSERT
    s.observe(11L) // CREATE INDEX
    s.observe(12L) // UPDATE
    assertEquals(SessionConsistencyToken(12), s.token)
    s.observe(11L)
    assertEquals(SessionConsistencyToken(12), s.token, "an older observation never lowers the token")
