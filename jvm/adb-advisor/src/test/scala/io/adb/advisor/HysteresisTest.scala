package io.adb.advisor

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the confirm/withdraw hysteresis and of the audit log. */
final class HysteresisTest:
  private val index = CreateIndex(7L, "orders", Vector(2L), Vector("customer_id"), Some(Score(10.0, 1.0)), Vector(1L))
  private val analyze = RunAnalyze(7L, "orders", "it was never analyzed", Vector(1L))
  private val never: Recommendation => Boolean = _ => false

  private def kinds(changes: Vector[(AuditKind, Recommendation)]) = changes.map(_._1)

  @Test def aFindingIsShownOnlyAfterConfirmAfterEvaluationsInARow(): Unit =
    val h = new Hysteresis(3, 3)
    assertTrue(h.update(Vector(index), never).isEmpty)
    assertTrue(h.update(Vector(index), never).isEmpty)
    assertEquals(Vector(AuditKind.Confirmed), kinds(h.update(Vector(index), never)))
    assertEquals(Vector(index), h.shown)
    assertTrue(h.update(Vector(index), never).isEmpty, "confirmed once, not every evaluation")

  @Test def aGapRestartsTheCountOfAnUnconfirmedFinding(): Unit =
    val h = new Hysteresis(3, 3)
    h.update(Vector(index), never); h.update(Vector(index), never)
    h.update(Vector.empty, never)
    h.update(Vector(index), never); h.update(Vector(index), never)
    assertTrue(h.shown.isEmpty, "two in a row after a gap is not three in a row")
    assertEquals(Vector(AuditKind.Confirmed), kinds(h.update(Vector(index), never)))

  @Test def aShownFindingSurvivesShortGapsAndIsWithdrawnAfterWithdrawAfterMisses(): Unit =
    val h = new Hysteresis(1, 3)
    h.update(Vector(index), never)
    assertTrue(h.update(Vector.empty, never).isEmpty)
    assertTrue(h.update(Vector.empty, never).isEmpty)
    assertTrue(h.update(Vector(index), never).isEmpty, "found again: the misses are forgotten")
    h.update(Vector.empty, never); h.update(Vector.empty, never)
    val last = h.update(Vector.empty, never)
    assertEquals(Vector(AuditKind.Withdrawn), kinds(last))
    assertTrue(h.shown.isEmpty)

  @Test def aCarriedOutAdviceIsWithdrawnAtOnceAsApplied(): Unit =
    val h = new Hysteresis(1, 3)
    h.update(Vector(index), never)
    val changes = h.update(Vector(index), r => r == index)
    assertEquals(Vector(AuditKind.Applied), kinds(changes))
    assertTrue(h.shown.isEmpty)

  @Test def theShownTextFollowsTheLatestEvaluation(): Unit =
    val h = new Hysteresis(1, 3)
    h.update(Vector(index), never)
    val better = index.copy(score = Some(Score(40.0, 1.0)))
    h.update(Vector(better), never)
    assertEquals(Vector(better), h.shown)

  @Test def independentFindingsAreTrackedSeparately(): Unit =
    val h = new Hysteresis(2, 2)
    h.update(Vector(index, analyze), never)
    val changes = h.update(Vector(index), never)
    assertEquals(Vector(index), changes.map(_._2))
    assertEquals(Vector(index), h.shown)

  @Test def theAuditLogKeepsTheNewestEntriesAndNumbersThemForever(): Unit =
    val log = new AuditLog(2)
    log.record(AuditKind.Confirmed, index, 10L)
    log.record(AuditKind.Withdrawn, index, 20L)
    val last = log.record(AuditKind.Applied, analyze, 30L)
    assertEquals(3L, last.sequence)
    assertEquals(2, log.size)
    assertEquals(Vector(2L, 3L), log.entries.map(_.sequence))
    assertEquals(Some("ANALYZE orders"), last.sql)
    assertEquals("applied", last.kind.label)
