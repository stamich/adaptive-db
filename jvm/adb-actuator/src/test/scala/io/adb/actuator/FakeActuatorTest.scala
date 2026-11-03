package io.adb.actuator

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the fake actuator the other tests rely on (and so of the contract it stands for). */
final class FakeActuatorTest:
  private val create = Action(ActionId(1), ActionKind.CreateIndex, ActionTarget(7L, "index:7:2"), "orders",
    "CREATE INDEX adv_orders_customer_id ON orders (customer_id)", Some("DROP INDEX adv_orders_customer_id"),
    Some("adv_orders_customer_id"), ActionState.Proposed, "", Vector.empty, 0L, 0L)

  @Test def itRecordsCallsAndReportsDone(): Unit =
    val fake = new FakeActuator
    assertEquals(Done, fake(create))
    assertEquals(Done, fake.undo(create))
    assertEquals(Vector(create), fake.applied.toVector)
    assertEquals(Vector(create), fake.undone.toVector)

  @Test def aScriptedFailureHappensOnceAndLeavesNoEffect(): Unit =
    val fake = new FakeActuator
    var effects = 0
    fake.onApply = _ => effects += 1
    fake.failNextApply = Some("table is locked")
    assertEquals(ActuationFailed("table is locked"), fake(create))
    assertEquals(0, effects, "a failed statement changes nothing")
    assertEquals(Done, fake(create))
    assertEquals(1, effects)

  @Test def undoNeedsAnUndoStatement(): Unit =
    val fake = new FakeActuator
    assertEquals(ActuationFailed("no undo statement"), fake.undo(create.copy(undoSql = None)))
