package io.adb.physical

import io.adb.model.*
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Wire-format tests of [[PlanJsonEncoder]] (plan wire v2). */
class PlanJsonEncoderTest:
  /** Every plan is wrapped in the version-2 envelope; operator and expression tags match the
    * Rust serde representation.
    */
  @Test def emitsEnvelopeAndRustSerdeTags(): Unit =
    val plan = PhysicalPlan.Filter(
      PhysicalPlan.EntityScan(EntityId(7), Vector(ScanColumn(FieldId(2), SlotId(0)))),
      PhysicalExpr.Binary(PhysicalExpr.Slot(SlotId(0)), PhysicalBinaryOp.Gt, PhysicalExpr.Literal(DbValue.Int64Value(100)))
    )
    assertEquals(
      "{\"wire_version\":2,\"plan\":{\"op\":\"filter\",\"input\":{\"op\":\"entity_scan\",\"entity_id\":7," +
        "\"columns\":[{\"field_id\":2,\"slot\":0}]},\"predicate\":{\"kind\":\"binary\",\"left\":{\"kind\":\"slot\",\"slot\":0}," +
        "\"op\":\"gt\",\"right\":{\"kind\":\"literal\",\"value\":{\"Int64\":100}}}}}",
      PlanJsonEncoder.encode(plan)
    )

  /** A composed 128-bit RowId is emitted as a decimal JSON string without precision loss. */
  @Test def encodesFullWidthRowIdAsString(): Unit =
    val rowId = (BigInt(7) << 64) | BigInt("18446744073709551615")
    val json = PlanJsonEncoder.encode(PhysicalPlan.PointLookup(rowId, Vector.empty))
    assertTrue(json.contains(s"\"row_id\":\"$rowId\""))

  /** Doubles that JSON cannot represent are rejected instead of producing invalid JSON. */
  @Test def rejectsNonFiniteDoubles(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => PlanJsonEncoder.encodeRustValue(DbValue.Float64Value(Double.NaN)))

  /** Cross-language contract: the encoded plan of a relational query is byte-identical to the
    * fixture that the Rust test `adb-plan-wire/tests/jvm_contract.rs` decodes and validates.
    * Regenerate the fixture only together with a deliberate wire change.
    */
  @Test def matchesTheNativeContractFixture(): Unit =
    val encoded = PlanJsonEncoder.encode(PlanningFixtures.planned(PlanningFixtures.ContractQuery).plan)
    val fixture = new String(Files.readAllBytes(PlanningFixtures.contractFixture), StandardCharsets.UTF_8).trim
    assertEquals(fixture, encoded)
