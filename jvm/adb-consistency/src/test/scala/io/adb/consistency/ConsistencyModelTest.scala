package io.adb.consistency

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The token, the levels and the read context. */
final class ConsistencyModelTest:
  /** A token only moves forward, and merging keeps the larger position. */
  @Test def tokensOnlyMoveForward(): Unit =
    val token = SessionConsistencyToken.Initial.advancedTo(5)
    assertEquals(SessionConsistencyToken(5), token)
    assertEquals(token, token.advancedTo(3))
    assertEquals(SessionConsistencyToken(9), token.merge(SessionConsistencyToken(9)))
    assertEquals(SessionConsistencyToken(9), SessionConsistencyToken(9).merge(token))
    assertTrue(token.isSatisfiedBy(5) && !token.isSatisfiedBy(4))
    assertThrows(classOf[IllegalArgumentException], () => SessionConsistencyToken(-1))

  /** Each level names the commit a projection must reach. */
  @Test def requiredCommitFollowsTheLevel(): Unit =
    val token = SessionConsistencyToken(7)
    def context(level: ReadConsistency) = SemanticReadContext(level, token, headCommitSeq = 12)
    assertEquals(Some(12L), context(ReadConsistency.StrongLatest).requiredCommitSeq)
    assertEquals(Some(7L), context(ReadConsistency.ReadYourWrites).requiredCommitSeq)
    assertEquals(None, context(ReadConsistency.Eventual).requiredCommitSeq)
    assertEquals(None, context(ReadConsistency.BoundedStaleness(StalenessBound.Millis(100))).requiredCommitSeq)

  /** A token from the future is detected. */
  @Test def aTokenAheadOfTheHeadIsDetected(): Unit =
    assertTrue(SemanticReadContext(ReadConsistency.Default, SessionConsistencyToken(20), 12).tokenIsAhead)
    assertFalse(SemanticReadContext(ReadConsistency.Default, SessionConsistencyToken(12), 12).tokenIsAhead)

  /** The SQL spellings. */
  @Test def sqlSpellings(): Unit =
    assertEquals("STRONG", ReadConsistency.StrongLatest.sql)
    assertEquals("READ_YOUR_WRITES", ReadConsistency.ReadYourWrites.sql)
    assertEquals("EVENTUAL", ReadConsistency.Eventual.sql)
    assertEquals("BOUNDED 250 MS", ReadConsistency.BoundedStaleness(StalenessBound.Millis(250)).sql)
    assertEquals("BOUNDED 3 COMMITS", ReadConsistency.BoundedStaleness(StalenessBound.Commits(3)).sql)
    assertThrows(classOf[IllegalArgumentException], () => StalenessBound.Millis(-1))
    assertEquals(ReadConsistency.ReadYourWrites, ReadConsistency.Default)

  /** A projection covers a commit only when ready and at or beyond it; the wait estimate uses the rate. */
  @Test def projectionCoverageAndEstimates(): Unit =
    def info(state: ProjectionState, at: Long, rate: Double) = ProjectionInfo(
      1, "p", ProjectionKind.TestAsync, ProjectionMode.Asynchronous, None, 1, state,
      ProjectionFrontier(at, at), 20, 20 - at, 0, 0, rate)
    assertTrue(info(ProjectionState.Ready, 10, 0).covers(10))
    assertFalse(info(ProjectionState.Ready, 10, 0).covers(11))
    assertFalse(info(ProjectionState.Building, 10, 0).covers(1))
    assertEquals(Some(0L), info(ProjectionState.Ready, 10, 0).estimatedMillisUntil(10))
    assertEquals(None, info(ProjectionState.Ready, 10, 0).estimatedMillisUntil(15))
    assertEquals(Some(500L), info(ProjectionState.Ready, 10, 10.0).estimatedMillisUntil(15))
    assertEquals(None, info(ProjectionState.Ready, 10, Double.NaN).estimatedMillisUntil(15))
