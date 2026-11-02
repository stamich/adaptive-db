package io.adb.consistency

import io.adb.consistency.FreshnessResolution.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The freshness rules. */
final class FreshnessResolverTest:
  /** A projection at `at`, with the given state and rate. */
  private def projection(at: Long, rate: Double = 0.0, state: ProjectionState = ProjectionState.Ready) =
    ProjectionInfo(1000, "index:ix", ProjectionKind.SecondaryIndex, ProjectionMode.Asynchronous, Some(1), 1, state,
      ProjectionFrontier(at, at), 20, 20 - at, 0, 0, rate)

  /** A context at head 20. */
  private def context(level: ReadConsistency, token: Long = 0, wait: Long = 1000) =
    SemanticReadContext(level, SessionConsistencyToken(token), 20, wait)

  /** A projection that has the session's writes is used directly. */
  @Test def readYourWritesIsDirectWhenCovered(): Unit =
    assertEquals(Direct, FreshnessResolver.resolve(context(ReadConsistency.ReadYourWrites, 15), projection(16), true))
    assertEquals(Direct, FreshnessResolver.resolve(context(ReadConsistency.ReadYourWrites, 0), projection(2), true))

  /** A projection that is slightly behind is waited for; one that is far behind is bypassed. */
  @Test def waitsOnlyWhenItIsQuick(): Unit =
    val ryw = context(ReadConsistency.ReadYourWrites, 15)
    assertEquals(WaitUntil(15, Some(500)), FreshnessResolver.resolve(ryw, projection(10, 10.0), true))
    assertEquals(Fallback("projection is behind commit 15"), FreshnessResolver.resolve(ryw, projection(10, 1.0), true))
    assertTrue(FreshnessResolver.resolve(ryw, projection(10, 1.0), false).isInstanceOf[Infeasible])

  /** With an unknown rate, a fallback is preferred and otherwise the read waits. */
  @Test def unknownRate(): Unit =
    val ryw = context(ReadConsistency.ReadYourWrites, 15)
    assertEquals(Fallback("projection is behind commit 15"), FreshnessResolver.resolve(ryw, projection(10), true))
    assertEquals(WaitUntil(15, None), FreshnessResolver.resolve(ryw, projection(10), false))

  /** Strong reads need the head. */
  @Test def strongNeedsTheHead(): Unit =
    val strong = context(ReadConsistency.StrongLatest)
    assertEquals(Direct, FreshnessResolver.resolve(strong, projection(20), true))
    assertEquals(WaitUntil(20, Some(200)), FreshnessResolver.resolve(strong, projection(18, 10.0), true))

  /** A projection that cannot be read is bypassed whatever the level. */
  @Test def unreadableProjectionsFallBack(): Unit =
    for state <- Seq(ProjectionState.Building, ProjectionState.NeedsRebuild, ProjectionState.Failed) do
      assertTrue(FreshnessResolver.resolve(context(ReadConsistency.Eventual), projection(20, state = state), true).isInstanceOf[Fallback])
      assertTrue(FreshnessResolver.resolve(context(ReadConsistency.Eventual), projection(20, state = state), false).isInstanceOf[Infeasible])

  /** A token from the future cannot be satisfied by waiting. */
  @Test def tokenBeyondTheHeadIsInfeasible(): Unit =
    val result = FreshnessResolver.resolve(context(ReadConsistency.ReadYourWrites, 99), projection(20), true)
    assertTrue(result.isInstanceOf[Infeasible], result.toString)
    // Eventual reads do not look at the token.
    assertEquals(Direct, FreshnessResolver.resolve(context(ReadConsistency.Eventual, 99), projection(20), true))

  /** Eventual reads accept any lag. */
  @Test def eventualAcceptsLag(): Unit =
    assertEquals(AllowStale(15), FreshnessResolver.resolve(context(ReadConsistency.Eventual), projection(5), true))

  /** Bounded reads: commit bounds compare the lag; time bounds need a rate. */
  @Test def boundedStaleness(): Unit =
    def bounded(b: StalenessBound) = context(ReadConsistency.BoundedStaleness(b))
    assertEquals(AllowStale(5), FreshnessResolver.resolve(bounded(StalenessBound.Commits(5)), projection(15), true))
    assertEquals(Fallback("staleness bound exceeded"), FreshnessResolver.resolve(bounded(StalenessBound.Commits(4)), projection(15), true))
    assertEquals(AllowStale(5), FreshnessResolver.resolve(bounded(StalenessBound.Millis(600)), projection(15, 10.0), true))
    // 5 commits at 10/s is 500 ms: over a 100 ms bound, but quick enough to wait for.
    assertEquals(WaitUntil(20, Some(500)), FreshnessResolver.resolve(bounded(StalenessBound.Millis(100)), projection(15, 10.0), true))
    // No rate: a time bound cannot be proven.
    assertEquals(Fallback("staleness bound exceeded"), FreshnessResolver.resolve(bounded(StalenessBound.Millis(600)), projection(15), true))
    assertEquals(Direct, FreshnessResolver.resolve(bounded(StalenessBound.Millis(0)), projection(20), true))
