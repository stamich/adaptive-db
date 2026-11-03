package io.adb.advisor

import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the design views the advisor reads. */
final class PhysicalDesignTest:
  private val index = IndexInfo(
    IndexId(1), "ix_orders_customer", EntityId(7),
    Vector(IndexKeyColumn(FieldId(2), descending = false), IndexKeyColumn(FieldId(3), descending = false)),
    Vector.empty, 1L, 1L, 1L
  )
  private val orders = EntityDesign(7L, "orders", Map(1L -> "id", 2L -> "customer_id", 3L -> "amount"), 1000L, None, Vector(index))

  @Test def leadingIndexMatchesOnlyTheFirstKeyColumn(): Unit =
    assertTrue(orders.hasLeadingIndexOn(2L))
    assertFalse(orders.hasLeadingIndexOn(3L), "the second key column does not serve a filter alone")
    assertFalse(orders.hasLeadingIndexOn(1L))

  @Test def unknownFieldsGetAPlaceholderName(): Unit =
    assertEquals("amount", orders.fieldName(3L))
    assertEquals("f99", orders.fieldName(99L))

  @Test def designLooksEntitiesUpById(): Unit =
    val design = PhysicalDesign.of(Seq(orders))
    assertEquals(Some(orders), design.entity(7L))
    assertEquals(None, design.entity(8L))
    assertTrue(PhysicalDesign.empty.entities.isEmpty)
