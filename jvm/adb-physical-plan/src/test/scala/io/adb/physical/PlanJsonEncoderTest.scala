package io.adb.physical

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Wire-format tests of [[PlanJsonEncoder]]. */
class PlanJsonEncoderTest:
  /** Operator and expression tags match the Rust serde representation. */
  @Test def emitsRustSerdeTags(): Unit =
    val plan = PhysicalPlan.Filter(
      PhysicalPlan.Scan,
      PhysicalExpr.Binary(
        PhysicalExpr.Column(FieldId(2)),
        PhysicalBinaryOp.Gt,
        PhysicalExpr.Literal(DbValue.Int64Value(100))
      )
    )
    val json = PlanJsonEncoder.encode(plan)
    assertTrue(json.contains("\"op\":\"filter\""))
    assertTrue(json.contains("\"kind\":\"binary\""))
    assertTrue(json.contains("\"Int64\":100"))


  /** Verifies that a composed 128-bit RowId is emitted as a decimal JSON string without precision loss. */
  @Test def encodesFullWidthRowIdAsString(): Unit =
    val rowId = (BigInt(7) << 64) | BigInt("18446744073709551615")
    val json = PlanJsonEncoder.encode(PhysicalPlan.PointLookup(rowId))
    assertTrue(json.contains(s"\"row_id\":\"$rowId\""))

  /** Table scans are planned as native entity range scans, not filtered full scans (2.0.3). */
  @Test def tableScanBecomesEntityScan(): Unit =
    val json = PlanJsonEncoder.encode(PhysicalPlan.EntityScan(EntityId(7)))
    assertEquals("{\"op\":\"entity_scan\",\"entity_id\":7}", json)
