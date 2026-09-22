//! Regression tests for the Milestone 2.0.1 physical-plan JSON wire contract.

use adb_core::RowId;
use adb_execution::PhysicalPlan;

/// Verifies that full-width u128 row identifiers use a lossless decimal string on JSON wire.
#[test]
fn point_lookup_row_id_round_trips_full_u128() {
    let row_id = RowId((7u128 << 64) | u128::from(u64::MAX));
    let plan = PhysicalPlan::PointLookup { row_id };
    let json = serde_json::to_string(&plan).unwrap();

    assert!(json.contains("\"row_id\":\""));
    let decoded: PhysicalPlan = serde_json::from_str(&json).unwrap();
    match decoded {
        PhysicalPlan::PointLookup { row_id: actual } => assert_eq!(actual, row_id),
        _ => panic!("expected point lookup"),
    }
}

/// Verifies backward compatibility with the small numeric row-id form used by the original 2.0 test.
#[test]
fn point_lookup_accepts_legacy_numeric_row_id() {
    let decoded: PhysicalPlan =
        serde_json::from_str(r#"{"op":"point_lookup","row_id":1}"#).unwrap();
    match decoded {
        PhysicalPlan::PointLookup { row_id } => assert_eq!(row_id, RowId(1)),
        _ => panic!("expected point lookup"),
    }
}
