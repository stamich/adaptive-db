package io.adb.actuator

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the action log. */
final class ActionLogTest:
  @Test def entriesAreNumberedFromOneInOrder(): Unit =
    val log = new ActionLog(10)
    log.record(ActionEvent.Applied, 5L, Some(ActionId(1)), "create_index adv_x", "built")
    log.record(ActionEvent.Kept, 9L, Some(ActionId(1)), "create_index adv_x", "rows fell 100x")
    assertEquals(Vector(1L, 2L), log.entries.map(_.sequence))
    assertEquals(Vector(ActionEvent.Applied, ActionEvent.Kept), log.entries.map(_.event))

  @Test def theOldestEntriesAreDroppedButNumbersAreNeverReused(): Unit =
    val log = new ActionLog(2)
    for i <- 1 to 5 do log.record(ActionEvent.Rejected, i.toLong, None, "create_index adv_y", s"reason $i")
    assertEquals(2, log.size)
    assertEquals(Vector(4L, 5L), log.entries.map(_.sequence), "a reader can see that 1-3 were dropped")

  @Test def aRejectionHasNoActionId(): Unit =
    val log = new ActionLog(3)
    assertEquals(None, log.record(ActionEvent.Rejected, 1L, None, "create_index adv_z", "below_ratio").actionId)

  @Test def eventLabelsAreSnakeCase(): Unit =
    assertEquals("rolled_back", ActionEvent.RolledBack.label)
    assertEquals("externally_dropped", ActionEvent.ExternallyDropped.label)
    assertEquals("applied", ActionEvent.Applied.label)

  @Test def aNonPositiveCapacityIsRefused(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => new ActionLog(0))
