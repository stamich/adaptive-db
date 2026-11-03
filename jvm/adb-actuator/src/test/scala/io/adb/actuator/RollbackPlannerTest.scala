package io.adb.actuator

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of rollback planning, the ownership guard and the action history. */
final class RollbackPlannerTest:
  private val owned = OwnedIndex("adv_orders_customer_id", 7L, "orders", Vector(2L), ActionTarget(7L, "index:7:2"), Some(ActionId(1)), 0L, 1L)
  private def registry =
    val r = new OwnershipRegistry
    r.record(owned)
    r
  private def create(name: String) = Action(ActionId(1), ActionKind.CreateIndex, ActionTarget(7L, "index:7:2"), "orders",
    s"CREATE INDEX $name ON orders (customer_id)", Some(s"DROP INDEX $name"), Some(name), ActionState.Applied, "", Vector.empty, 0L, 0L)

  @Test def anOwnedIndexGetsADropAction(): Unit =
    val plan = RollbackPlanner.plan(registry, "adv_orders_customer_id", "no rows reduction", ActionId(9), 50L)
    val action = plan.toOption.get
    assertEquals(ActionKind.DropIndex, action.kind)
    assertEquals("DROP INDEX adv_orders_customer_id", action.sql)
    assertEquals(owned.target, action.target)
    assertEquals("no rows reduction", action.reason)
    assertEquals(None, action.undoSql, "a drop is not undone")

  @Test def aUserIndexGetsNoDropAction(): Unit =
    val plan = RollbackPlanner.plan(registry, "ix_orders_user", "idle", ActionId(9), 50L)
    assertTrue(plan.isLeft)
    assertTrue(plan.left.toOption.get.contains("not owned"))

  @Test def theGuardPassesOwnedDropsAndRefusesOthers(): Unit =
    val fake = new FakeActuator
    val guard = new OwnedOnlyActuator(fake, registry)
    val drop = RollbackPlanner.plan(registry, "adv_orders_customer_id", "x", ActionId(2), 0L).toOption.get
    assertEquals(Done, guard(drop))
    assertEquals(1, fake.applied.size)
    val foreign = drop.copy(sql = "DROP INDEX ix_orders_user", indexName = Some("ix_orders_user"))
    assertTrue(guard(foreign).isInstanceOf[ActuationFailed])
    assertEquals(1, fake.applied.size, "the engine was not reached")

  @Test def theGuardRefusesToUndoAnIndexItDoesNotOwn(): Unit =
    val fake = new FakeActuator
    val guard = new OwnedOnlyActuator(fake, registry)
    assertEquals(Done, guard.undo(create("adv_orders_customer_id")))
    assertTrue(guard.undo(create("ix_orders_user")).isInstanceOf[ActuationFailed])
    assertEquals(1, fake.undone.size)

  @Test def theGuardPassesCreatesAndAnalyzeThrough(): Unit =
    val fake = new FakeActuator
    val guard = new OwnedOnlyActuator(fake, new OwnershipRegistry)
    assertEquals(Done, guard(create("adv_new")))
    assertEquals(Done, guard(create("adv_new").copy(kind = ActionKind.Analyze, indexName = None)))
    assertEquals(2, fake.applied.size)

  @Test def aDropWithoutAnIndexNameIsRefused(): Unit =
    val guard = new OwnedOnlyActuator(new FakeActuator, registry)
    assertTrue(guard(create("x").copy(kind = ActionKind.DropIndex, indexName = None)).isInstanceOf[ActuationFailed])

  @Test def historyRemembersActionsAndRollbacksPerTarget(): Unit =
    val h = new ActionHistory
    h.recordAction(owned.target, 10L)
    assertEquals(Some(10L), h.lastAction("index:7:2"))
    assertEquals(None, h.lastRollback("index:7:2"))
    h.recordRollback(owned.target, 99L)
    assertEquals(Some(99L), h.lastAction("index:7:2"), "a rollback is an action too")
    assertEquals(Some(99L), h.lastRollback("index:7:2"))
    assertEquals(1, h.rollbacks("index:7:2"))
    assertEquals(0, h.rollbacks("index:7:3"))
