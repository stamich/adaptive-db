package io.adb.physical

import io.adb.model.*
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Wire-format tests of [[PlanJsonEncoder]] (plan wire v3). */
class PlanJsonEncoderTest:
  /** Every plan is wrapped in the version-3 envelope; operator and expression tags match the
    * Rust serde representation.
    */
  @Test def emitsEnvelopeAndRustSerdeTags(): Unit =
    val plan = PhysicalPlan.Filter(
      PhysicalPlan.EntityScan(EntityId(7), Vector(ScanColumn(FieldId(2), SlotId(0)))),
      PhysicalExpr.Binary(PhysicalExpr.Slot(SlotId(0)), PhysicalBinaryOp.Gt, PhysicalExpr.Literal(DbValue.Int64Value(100)))
    )
    assertEquals(
      "{\"wire_version\":3,\"plan\":{\"op\":\"filter\",\"input\":{\"op\":\"entity_scan\",\"entity_id\":7," +
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

  /** Cross-language contract for the index operators of plan wire v3: the encoded plan is
    * byte-identical to the fixture that `adb-plan-wire/tests/jvm_contract.rs` decodes.
    */
  @Test def matchesTheNativeIndexFixture(): Unit =
    val scan = PhysicalPlan.IndexScan(
      IndexId(4),
      IndexBounds(
        Vector(DbValue.StringValue("Kraków")),
        Some(ValueBound(DbValue.Int64Value(-5), inclusive = false)),
        Some(ValueBound(DbValue.Int64Value(100), inclusive = true))
      ),
      ScanDirection.Backward,
      Vector(ScanColumn(FieldId(2), SlotId(0)), ScanColumn(FieldId(3), SlotId(1))),
      Some(10)
    )
    val plan = PhysicalPlan.Project(
      PhysicalPlan.RowFetch(
        PhysicalPlan.Filter(scan, PhysicalExpr.Binary(PhysicalExpr.Slot(SlotId(1)), PhysicalBinaryOp.Ne, PhysicalExpr.Literal(DbValue.Int64Value(7)))),
        Vector(ScanColumn(FieldId(4), SlotId(2)))
      ),
      Vector(SlotId(2), SlotId(1))
    )
    val fixture = PlanningFixtures.contractFixture.resolveSibling("jvm-index-plan.json")
    assertEquals(new String(Files.readAllBytes(fixture), StandardCharsets.UTF_8).trim, PlanJsonEncoder.encode(plan))
    assertEquals(Vector(scan), PhysicalPlan.preorder(plan).collect { case s: PhysicalPlan.IndexScan => s })
    val text = Explain.physical(plan, Map(SlotId(0) -> "balance"))
    assertTrue(text.contains("IndexScan index=4 prefix=[{\"String\":\"Kraków\"}] range=({\"Int64\":-5}, {\"Int64\":100}] backward limit=10"), text)
    assertTrue(text.contains("RowFetch [f4->#2]"), text)

