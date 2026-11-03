package io.adb.actuator

import io.adb.advisor.*
import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the policy gate: every rejection reason, approval, and the absence of side effects. */
final class PolicyGateTest:
  private val second = 1_000_000_000L
  private val policy = Policy()
  private val budget = Budget()
  private val now = 10_000L * second

  private val recommendation = CreateIndex(7L, "orders", Vector(2L), Vector("customer_id"), Some(Score(500.0, 100.0)), Vector(11L))
  private val design = PhysicalDesign.of(Seq(EntityDesign(7L, "orders", Map(2L -> "customer_id"), 40_000L, None, Vector.empty)))
  /** A workload in which shape 11 ran `executions` times and scanned `rowsRead` rows per execution. */
  private def workload(executions: Long = 200L, rowsRead: Long = 40_000L) =
    WorkloadSnapshot(Vector.empty, Map(11L -> ShapeDelta(11L, executions, 0L, executions * 1_000_000L, executions, executions * rowsRead, 1.0)))
  private def decide(r: Recommendation = recommendation, d: PhysicalDesign = design, w: WorkloadSnapshot = workload(),
                     p: Policy = policy, b: Budget = budget, c: GateContext = GateContext(now)) =
    PolicyGate.decide(r, d, w, p, b, c)
  private def reason(decision: GateDecision): Option[RejectReason] = decision match
    case Reject(why, _) => Some(why)
    case Approve => None

  @Test def aWellEvidencedRecommendationInsideTheBudgetIsApproved(): Unit =
    assertEquals(Approve, decide())

  @Test def informationIsNotActionable(): Unit =
    assertEquals(Some(RejectReason.NotActionable), reason(decide(r = Informational("hot-join", "k", "text", Vector(11L)))))

  @Test def anIndexThatAlreadyExistsIsRefused(): Unit =
    val info = IndexInfo(IndexId(1), "mine", EntityId(7), Vector(IndexKeyColumn(FieldId(2), descending = false)), Vector.empty, 1L, 1L, 1L)
    val indexed = PhysicalDesign.of(Seq(EntityDesign(7L, "orders", Map(2L -> "customer_id"), 40_000L, None, Vector(info))))
    assertEquals(Some(RejectReason.AlreadyExists), reason(decide(d = indexed)))

  @Test def tooFewExecutionsAreRefused(): Unit =
    assertEquals(Some(RejectReason.TooFewExecutions), reason(decide(w = workload(executions = 49L))))
    assertEquals(Approve, decide(w = workload(executions = 50L)))

  @Test def aLowRatioIsRefused(): Unit =
    val low = recommendation.copy(score = Some(Score(300.0, 100.0)))
    assertEquals(Some(RejectReason.BelowRatio), reason(decide(r = low)))
    assertEquals(Some(RejectReason.BelowRatio), reason(decide(r = recommendation.copy(score = None))))

  @Test def anIndexAPersonDroppedIsNotRecreated(): Unit =
    assertEquals(Some(RejectReason.ExternallyDropped), reason(decide(c = GateContext(now, externallyDropped = true))))

  @Test def aRecentActionOnTheTargetIsInCooldown(): Unit =
    assertEquals(Some(RejectReason.Cooldown), reason(decide(c = GateContext(now, lastActionNanos = Some(now - 29L * 60L * second)))))
    assertEquals(Approve, decide(c = GateContext(now, lastActionNanos = Some(now - 31L * 60L * second))))

  @Test def aRecentRollbackBlocksTheTargetLonger(): Unit =
    val c = GateContext(now, lastActionNanos = Some(now - 31L * 60L * second), lastRollbackNanos = Some(now - 5L * 3600L * second))
    assertEquals(Some(RejectReason.RollbackCooldown), reason(decide(c = c)))
    assertEquals(Approve, decide(c = c.copy(lastRollbackNanos = Some(now - 7L * 3600L * second))))

  @Test def theIndexCountLimitIsEnforced(): Unit =
    assertEquals(Some(RejectReason.TooManyIndexes), reason(decide(c = GateContext(now, ownedIndexes = 8))))
    assertEquals(Approve, decide(c = GateContext(now, ownedIndexes = 7)))

  @Test def aTableAboveTheBuildCeilingNeedsAPerson(): Unit =
    val big = PhysicalDesign.of(Seq(EntityDesign(7L, "orders", Map(2L -> "customer_id"), 200_000L, None, Vector.empty)))
    assertEquals(Some(RejectReason.TableTooLarge), reason(decide(d = big)))

  @Test def theNeverAnalyzedTableIsJudgedByTheRowsItsScansRead(): Unit =
    val unknown = PhysicalDesign.of(Seq(EntityDesign(7L, "orders", Map(2L -> "customer_id"), 0L, None, Vector.empty)))
    assertEquals(Some(RejectReason.TableTooLarge), reason(decide(d = unknown, w = workload(rowsRead = 200_000L))))

  @Test def theByteBudgetIsEnforced(): Unit =
    val tight = Budget(maxIndexBytes = 1_000_000L)
    assertEquals(Some(RejectReason.TooManyBytes), reason(decide(b = tight)))
    assertEquals(Some(RejectReason.TooManyBytes), reason(decide(b = Budget(maxIndexBytes = 6_000_000L), c = GateContext(now, ownedBytes = 5_000_000L))))
    assertEquals(Approve, decide(b = Budget(maxIndexBytes = 6_000_000L)))

  @Test def theBuildLimitIsEnforced(): Unit =
    assertEquals(Some(RejectReason.BuildInFlight), reason(decide(c = GateContext(now, buildsInFlight = 1))))

  @Test def analyzeNeedsEvidenceAndCooldownButNoIndexBudget(): Unit =
    val analyze = RunAnalyze(7L, "orders", "stale", Vector(11L))
    val full = GateContext(now, ownedIndexes = 8, ownedBytes = Long.MaxValue / 2, buildsInFlight = 5)
    assertEquals(Approve, decide(r = analyze, c = full))
    assertEquals(Some(RejectReason.TooFewExecutions), reason(decide(r = analyze, w = workload(executions = 10L))))
    assertEquals(Some(RejectReason.Cooldown), reason(decide(r = analyze, c = GateContext(now, lastActionNanos = Some(now - second)))))

  @Test def theFirstFailingRuleNamesTheReason(): Unit =
    // Both too few executions and a low ratio: the evidence rule comes first.
    val low = recommendation.copy(score = Some(Score(1.0, 100.0)))
    assertEquals(Some(RejectReason.TooFewExecutions), reason(decide(r = low, w = workload(executions = 1L))))

  @Test def theGateIsPureSoTheSameInputsGiveTheSameAnswerAndTheRejectionCarriesWords(): Unit =
    val c = GateContext(now, ownedIndexes = 8)
    val first = decide(c = c)
    assertEquals(first, decide(c = c))
    assertTrue(first.asInstanceOf[Reject].detail.contains("limit 8"))

  @Test def reasonLabelsAreSnakeCase(): Unit =
    assertEquals("too_few_executions", RejectReason.TooFewExecutions.label)
    assertEquals("build_in_flight", RejectReason.BuildInFlight.label)
    assertEquals("externally_dropped", RejectReason.ExternallyDropped.label)
