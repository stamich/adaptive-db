package io.adb.actuator

import io.adb.advisor.{EntityDesign, PhysicalDesign}
import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the ownership registry: what the advisor may drop, and what it must leave alone. */
final class OwnershipRegistryTest:
  private def info(id: Long, name: String, field: Int = 2) =
    IndexInfo(IndexId(id), name, EntityId(7), Vector(IndexKeyColumn(FieldId(field), descending = false)), Vector.empty, 1L, 1L, 1L)
  private def design(infos: IndexInfo*) =
    PhysicalDesign.of(Seq(EntityDesign(7L, "orders", Map(2L -> "customer_id"), 1000L, None, infos.toVector)))
  private val mine = OwnedIndex("adv_orders_customer_id", 7L, "orders", Vector(2L), ActionTarget(7L, "index:7:2"), Some(ActionId(1)), 10L, 48_000L)

  @Test def onlyARecordedIndexMayBeDropped(): Unit =
    val r = new OwnershipRegistry
    assertFalse(r.mayDrop("adv_orders_customer_id"), "a name with the prefix is not enough")
    r.record(mine)
    assertTrue(r.mayDrop("adv_orders_customer_id"))
    assertTrue(r.mayDrop("ADV_ORDERS_CUSTOMER_ID"), "index names are case-insensitive")
    assertFalse(r.mayDrop("ix_user_index"))

  @Test def countAndBytesFollowTheRecords(): Unit =
    val r = new OwnershipRegistry
    r.record(mine); r.record(mine.copy(name = "adv_orders_amount", target = ActionTarget(7L, "index:7:3"), estimatedBytes = 100L))
    assertEquals(2, r.count)
    assertEquals(48_100L, r.bytes)
    assertEquals(Some(mine.name), r.forTarget(mine.target).map(_.name))

  @Test def releasingForgetsTheIndexAndReturnsIt(): Unit =
    val r = new OwnershipRegistry
    r.record(mine)
    assertEquals(Some(mine), r.release("adv_orders_customer_id"))
    assertFalse(r.mayDrop("adv_orders_customer_id"))
    assertEquals(None, r.release("adv_orders_customer_id"))

  @Test def anOwnedIndexThatVanishedWasDroppedByAPersonAndIsNotRecreated(): Unit =
    val r = new OwnershipRegistry
    r.record(mine)
    assertTrue(r.reconcile(design(info(1, "adv_orders_customer_id"))).isEmpty, "still there: nothing to do")
    val missing = r.reconcile(design())
    assertEquals(Vector(mine), missing)
    assertEquals(0, r.count)
    assertTrue(r.externallyDropped("index:7:2"))

  @Test def aUserIndexIsNeverAffectedByReconciliation(): Unit =
    val r = new OwnershipRegistry
    r.record(mine)
    r.reconcile(design(info(1, "adv_orders_customer_id"), info(2, "ix_orders_user", field = 3)))
    assertEquals(1, r.count)
    assertFalse(r.mayDrop("ix_orders_user"))

  @Test def recordingAgainClearsTheExternalDropMark(): Unit =
    val r = new OwnershipRegistry
    r.record(mine); r.reconcile(design())
    assertTrue(r.externallyDropped("index:7:2"))
    r.record(mine)
    assertFalse(r.externallyDropped("index:7:2"))

  @Test def indexesWithThePrefixAreAdoptedAfterARestartAndOthersAreNot(): Unit =
    val r = new OwnershipRegistry
    val adopted = r.adopt(design(info(1, "adv_orders_customer_id"), info(2, "ix_orders_user", field = 3)), 99L, Budget())
    assertEquals(Vector("adv_orders_customer_id"), adopted.map(_.name))
    assertTrue(adopted.head.adopted && adopted.head.createdBy.isEmpty)
    assertEquals(ActionTarget(7L, "index:7:2"), adopted.head.target, "the target matches the one CreateIndex produces")
    assertEquals(1000L * 48L, adopted.head.estimatedBytes)
    assertTrue(r.mayDrop("adv_orders_customer_id"))
    assertFalse(r.mayDrop("ix_orders_user"))

  @Test def adoptionIsIdempotentAndKeepsAnIndexThatIsAlreadyRecorded(): Unit =
    val r = new OwnershipRegistry
    r.record(mine)
    assertTrue(r.adopt(design(info(1, "adv_orders_customer_id")), 99L, Budget()).isEmpty)
    assertFalse(r.get("adv_orders_customer_id").get.adopted, "the original record is kept")
    val again = new OwnershipRegistry
    again.adopt(design(info(1, "adv_orders_customer_id")), 1L, Budget())
    assertTrue(again.adopt(design(info(1, "adv_orders_customer_id")), 2L, Budget()).isEmpty)
    assertEquals(1, again.count)

  @Test def thePrefixMatchIgnoresCase(): Unit =
    val r = new OwnershipRegistry
    assertEquals(1, r.adopt(design(info(1, "ADV_Orders_X")), 1L, Budget()).size)

