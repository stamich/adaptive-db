package io.adb.actuator

import io.adb.advisor.CreateIndex
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The whole loop over a scripted workload and a fake actuator. */
final class AdaptiveControllerTest:
  private def autoSafe(rig: Rig): Rig =
    rig.controller.mode = AdaptiveMode.AutoSafe
    rig

  @Test def offByDefaultDoesNothingNotEvenEvaluate(): Unit =
    val rig = new Rig
    assertEquals(AdaptiveMode.Off, rig.controller.mode)
    val result = rig.step()
    assertEquals(None, result.report)
    rig.steps(10)
    assertTrue(rig.fake.applied.isEmpty && rig.fake.undone.isEmpty)
    assertEquals(0, rig.controller.log.size)

  @Test def advisoryEvaluatesButNeverActs(): Unit =
    val rig = new Rig
    rig.controller.mode = AdaptiveMode.Advisory
    assertTrue(rig.step().report.isDefined)
    rig.steps(10)
    assertTrue(rig.advisor.lastReport.exists(_.recommendations.exists(_.isInstanceOf[CreateIndex])), "the advice is there")
    assertTrue(rig.fake.applied.isEmpty, "but nothing was done")
    assertEquals(0, rig.controller.log.size)

  @Test def autoSafeCreatesTheIndexOnTheThirdTickAndNothingElse(): Unit =
    val rig = autoSafe(new Rig)
    rig.step()                                    // baseline
    rig.step()                                    // seen once of two
    assertTrue(rig.fake.applied.isEmpty)
    rig.step()                                    // confirmed, gated, applied
    assertEquals(1, rig.fake.applied.size)
    val action = rig.fake.applied.head
    assertEquals(ActionKind.CreateIndex, action.kind)
    assertEquals("CREATE INDEX adv_orders_customer_id ON orders (customer_id)", action.sql)
    assertTrue(rig.controller.registry.mayDrop("adv_orders_customer_id"))
    assertEquals(Vector(ActionEvent.Applied), rig.events)
    assertEquals(ActionState.Applied, rig.controller.actions.head.state)

  @Test def anIndexThatCutsRowsReadIsKeptAfterTheVerificationWindows(): Unit =
    val rig = autoSafe(new Rig)
    rig.steps(3)
    rig.steps(2)
    assertEquals(ActionState.Applied, rig.controller.actions.head.state, "two of three windows")
    rig.step()
    assertEquals(ActionState.Kept, rig.controller.actions.head.state)
    assertTrue(rig.events.contains(ActionEvent.Kept))
    assertTrue(rig.fake.undone.isEmpty && !rig.fake.applied.exists(_.kind == ActionKind.DropIndex))
    assertEquals(1, rig.controller.registry.count)

  @Test def anIndexThatDoesNotReduceRowsReadIsRolledBack(): Unit =
    val rig = autoSafe(new Rig)
    rig.indexHelps = false
    rig.steps(3)
    rig.steps(3)
    assertEquals(ActionState.RolledBack, rig.controller.actions.find(_.kind == ActionKind.CreateIndex).get.state)
    val drop = rig.fake.applied.last
    assertEquals(ActionKind.DropIndex, drop.kind)
    assertEquals("DROP INDEX adv_orders_customer_id", drop.sql)
    assertEquals(0, rig.controller.registry.count)
    assertTrue(rig.indexes.isEmpty, "the schema is back as before")
    assertEquals(Vector(ActionEvent.Applied, ActionEvent.RolledBack), rig.events)
    assertTrue(rig.controller.log.entries.last.detail.contains("no_rows_reduction"))

  @Test def theRolledBackTargetIsNotRecreatedDuringTheCooldownAndTheRejectionIsLoggedOnce(): Unit =
    val rig = autoSafe(new Rig)
    rig.indexHelps = false
    rig.steps(6)                                  // created, then rolled back at the sixth tick
    val creates = () => rig.fake.applied.count(_.kind == ActionKind.CreateIndex)
    assertEquals(1, creates())
    rig.steps(20)                                 // 400 s, inside the 600 s rollback cooldown... and beyond the advice returning
    assertTrue(creates() <= 2)
    val rejections = rig.controller.log.entries.filter(_.event == ActionEvent.Rejected)
    assertTrue(rejections.nonEmpty)
    assertTrue(rejections.head.detail.startsWith("rollback_cooldown"), rejections.head.detail)
    assertEquals(rejections.map(_.detail).distinct.size, rejections.size, "the same rejection is not logged every tick")

  @Test def theBudgetRejectsASecondIndexWithAnAuditedReason(): Unit =
    val rig = new Rig(budget = Budget(maxAdvisorIndexes = 1))
    autoSafe(rig)
    rig.Load2(wantsIndex = false)                 // a second candidate, on amount
    rig.steps(5)
    assertEquals(1, rig.fake.applied.count(_.kind == ActionKind.CreateIndex))
    val rejection = rig.controller.log.entries.find(_.event == ActionEvent.Rejected)
    assertTrue(rejection.exists(_.detail.startsWith("too_many_indexes")), rig.controller.log.entries.toString)

  @Test def aFailedStatementLeavesNothingBehindAndIsNotRetriedAtOnce(): Unit =
    val rig = autoSafe(new Rig)
    rig.fake.failNextApply = Some("lock timeout")
    rig.steps(3)
    assertEquals(ActionState.Failed, rig.controller.actions.head.state)
    assertEquals("lock timeout", rig.controller.actions.head.reason)
    assertEquals(0, rig.controller.registry.count)
    assertTrue(rig.indexes.isEmpty)
    rig.step()
    assertEquals(1, rig.fake.applied.size, "inside the cooldown the advisor does not try again")
    assertTrue(rig.events.contains(ActionEvent.Failed))
    assertTrue(rig.controller.log.entries.exists(e => e.event == ActionEvent.Rejected && e.detail.startsWith("cooldown")))

  @Test def switchingToAdvisoryStopsNewActionsAtOnce(): Unit =
    val rig = autoSafe(new Rig)
    rig.steps(2)
    rig.controller.mode = AdaptiveMode.Advisory
    rig.steps(10)
    assertTrue(rig.fake.applied.isEmpty)
    rig.controller.mode = AdaptiveMode.AutoSafe
    rig.step()
    assertEquals(1, rig.fake.applied.size, "back in auto_safe the confirmed advice is carried out")

  @Test def switchingToAdvisoryAlsoStopsRollbacks(): Unit =
    val rig = autoSafe(new Rig)
    rig.indexHelps = false
    rig.steps(3)
    rig.controller.mode = AdaptiveMode.Advisory
    rig.steps(10)
    assertTrue(rig.fake.applied.forall(_.kind == ActionKind.CreateIndex), "no drop while only advising")
    assertEquals(1, rig.controller.registry.count)

  @Test def aUserIndexIsNeverTouchedEvenWhenIdle(): Unit =
    val rig = autoSafe(new Rig)
    rig.indexes = Vector(rig.indexInfo("ix_orders_amount", 3))
    rig.load.active = false
    rig.steps(30)
    assertTrue(rig.fake.applied.isEmpty && rig.fake.undone.isEmpty)
    assertEquals(0, rig.controller.registry.count)

  @Test def anIndexAPersonDroppedIsNotRecreated(): Unit =
    val rig = autoSafe(new Rig)
    rig.steps(6)                                  // created and kept
    assertEquals(ActionState.Kept, rig.controller.actions.head.state)
    rig.indexes = Vector.empty                    // a person drops it
    rig.load.rowsPerRun = 100_000L; rig.load.nanosPerRun = 100_000_000L
    rig.steps(12)
    assertTrue(rig.events.contains(ActionEvent.ExternallyDropped))
    assertEquals(1, rig.fake.applied.count(_.kind == ActionKind.CreateIndex), "not recreated")
    assertTrue(rig.controller.log.entries.exists(e => e.event == ActionEvent.Rejected && e.detail.startsWith("externally_dropped")))

  @Test def anAdvisorIndexNobodyWantsAnymoreIsDroppedAsIdle(): Unit =
    val rig = autoSafe(new Rig)
    rig.steps(6)                                  // created and kept
    rig.Load2(wantsIndex = false, rowsPerRun = 10L) // the workload moves to a query the index does not serve (and needs no index)
    rig.load.active = false
    rig.steps(4)                                  // idleWindows = 4
    assertTrue(rig.fake.applied.exists(a => a.kind == ActionKind.DropIndex), rig.controller.log.entries.toString)
    assertEquals(0, rig.controller.registry.count)
    assertTrue(rig.controller.log.entries.last.detail.contains("idle"))

  @Test def anIndexFoundAfterARestartIsAdoptedAndLaterDroppedWhenIdle(): Unit =
    val rig = autoSafe(new Rig)
    rig.indexes = Vector(rig.indexInfo("adv_orders_customer_id", 2))
    rig.load.rowsPerRun = 10L
    rig.step()
    assertTrue(rig.events.contains(ActionEvent.Adopted))
    assertTrue(rig.controller.registry.get("adv_orders_customer_id").exists(_.adopted))
    rig.load.active = false
    rig.Load2(wantsIndex = false, rowsPerRun = 10L)
    rig.steps(5)
    assertTrue(rig.fake.applied.exists(_.kind == ActionKind.DropIndex))

  @Test def aWriteRegressionRollsTheIndexBackEvenWhenReadsImproved(): Unit =
    val rig = autoSafe(new Rig)
    rig.writesPerStep = 2000L
    rig.writePenalty = 40.0                       // writes get 40x slower with the index
    rig.load.nanosPerRun = 2_000_000L             // reads are cheap already, so the read gain is small
    rig.steps(3)
    rig.steps(3)
    val create = rig.controller.actions.find(_.kind == ActionKind.CreateIndex).get
    assertEquals(ActionState.RolledBack, create.state)
    assertTrue(create.reason.startsWith("write_regression"), create.reason)

  @Test def analyzeIsAppliedOnceAndNeedsNoVerification(): Unit =
    val rig = autoSafe(new Rig)
    rig.load.active = false
    rig.Load2(entity = 8L, qError = 50.0, rowsPerRun = 10L)
    rig.steps(4)
    val analyze = rig.fake.applied.find(_.kind == ActionKind.Analyze)
    assertEquals("ANALYZE items", analyze.get.sql)
    assertEquals(ActionState.Kept, rig.controller.actions.find(_.kind == ActionKind.Analyze).get.state)
    rig.step()                                    // 40 s after the analyze, cooldown is 60 s
    assertEquals(1, rig.fake.applied.count(_.kind == ActionKind.Analyze), "the cooldown holds it back")

  @Test def closingTheControllerRefusesNewBuilds(): Unit =
    val rig = autoSafe(new Rig)
    rig.controller.close()
    rig.steps(6)
    assertTrue(rig.fake.applied.isEmpty)
    assertTrue(rig.controller.log.entries.exists(_.event == ActionEvent.Rejected))

  @Test def theOnlyChangesToTheDatabaseGoThroughTheActuator(): Unit =
    val rig = autoSafe(new Rig)
    rig.indexHelps = false
    rig.steps(8)
    val statements = (rig.fake.applied ++ rig.fake.undone).map(_.sql)
    assertTrue(statements.forall(s => s.startsWith("CREATE INDEX adv_") || s.startsWith("DROP INDEX adv_") || s.startsWith("ANALYZE ")), statements.toString)
