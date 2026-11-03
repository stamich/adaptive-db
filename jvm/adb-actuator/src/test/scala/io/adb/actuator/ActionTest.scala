package io.adb.actuator

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the action model. */
final class ActionTest:
  private val action = Action(
    ActionId(1), ActionKind.CreateIndex, ActionTarget(7L, "index:7:2"), "orders",
    "CREATE INDEX adv_orders_customer_id ON orders (customer_id)", Some("DROP INDEX adv_orders_customer_id"),
    Some("adv_orders_customer_id"), ActionState.Proposed, "", Vector(11L), 100L, 100L)

  @Test def movingAnActionChangesStateReasonAndTimeOnly(): Unit =
    val applied = action.moved(ActionState.Applied, "built", 250L)
    assertEquals(ActionState.Applied, applied.state)
    assertEquals("built", applied.reason)
    assertEquals(250L, applied.updatedAtNanos)
    assertEquals(100L, applied.proposedAtNanos)
    assertEquals(action.sql, applied.sql)

  @Test def onlyProposedAndAppliedAreNotFinal(): Unit =
    val open = ActionState.values.filterNot(_.isFinal).toSet
    assertEquals(Set(ActionState.Proposed, ActionState.Applied), open)

  @Test def labelsAreLowerCaseForReports(): Unit =
    assertEquals("create_index", ActionKind.CreateIndex.label)
    assertEquals("drop_index", ActionKind.DropIndex.label)
    assertEquals("rolled_back", ActionState.RolledBack.label)

  @Test def theSummaryNamesTheIndexOrElseTheTable(): Unit =
    assertEquals("create_index adv_orders_customer_id", action.summary)
    assertEquals("analyze orders", action.copy(kind = ActionKind.Analyze, indexName = None).summary)
