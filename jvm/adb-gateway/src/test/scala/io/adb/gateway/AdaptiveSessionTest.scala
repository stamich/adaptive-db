package io.adb.gateway

import io.adb.consistency.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Session state, which needs no native library: the token, the level and the read context. */
final class AdaptiveSessionTest:
  /** A backend whose head the test moves. */
  private final class FakeBackend(var head: Long) extends ConsistencyBackend:
    /** @inheritdoc */
    def latestCommitSeq(): Long = head
    /** @inheritdoc */
    def projections(): Vector[ProjectionInfo] = Vector.empty
    /** @inheritdoc */
    def awaitProjection(id: Long, commitSeq: Long, timeoutMillis: Long): Boolean = true

  /** A session over `backend`; its database is never used by these tests. */
  private def session(backend: ConsistencyBackend) = new AdaptiveSession(null, backend)

  /** A new session reads its own writes and has seen nothing. */
  @Test def startsWithDefaults(): Unit =
    val s = session(new FakeBackend(5))
    assertEquals(ReadConsistency.ReadYourWrites, s.consistency)
    assertEquals(SessionConsistencyToken.Initial, s.token)
    assertEquals(SemanticReadContext(ReadConsistency.ReadYourWrites, SessionConsistencyToken.Initial, 5, SemanticReadContext.DefaultMaxWaitMillis), s.readContext())

  /** The token only moves forward and the context follows the head. */
  @Test def tokenMovesForward(): Unit =
    val backend = new FakeBackend(5)
    val s = session(backend)
    s.observe(4L)
    s.observe(2L)
    assertEquals(SessionConsistencyToken(4), s.token)
    s.observe(SessionConsistencyToken(9))
    assertEquals(SessionConsistencyToken(9), s.token)
    backend.head = 11
    assertEquals(11L, s.readContext().headCommitSeq)
    assertTrue(!s.readContext().tokenIsAhead)

  /** Sessions do not share tokens. */
  @Test def sessionsAreIndependent(): Unit =
    val backend = new FakeBackend(5)
    val (a, b) = (session(backend), session(backend))
    a.observe(5L)
    assertEquals(SessionConsistencyToken(5), a.token)
    assertEquals(SessionConsistencyToken.Initial, b.token)

  /** The level and the wait limit are changeable; the wait limit is bounded. */
  @Test def levelAndWaitAreSettable(): Unit =
    val s = session(new FakeBackend(1))
    s.consistency = ReadConsistency.Eventual
    s.maxWaitMillis = 50
    val context = s.readContext()
    assertEquals((ReadConsistency.Eventual, 50L), (context.consistency, context.maxWaitMillis))
    assertThrows(classOf[IllegalArgumentException], () => { s.maxWaitMillis = ConsistencyBackend.MaxWaitMillis + 1 })
    assertThrows(classOf[IllegalArgumentException], () => { s.maxWaitMillis = -1 })
