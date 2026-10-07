//! Physical-plan JSON contract (serde form of the plan nodes, u128 row ids, slot mappings).
//!
//! The versioned envelope and size limits live in `adb-plan-wire`; this file covers the node
//! encoding that the envelope carries.
use adb_core::RowId;
use adb_execution::{PhysicalPlan, ScanColumn, SlotId};

/// Full-width u128 row identifiers use a lossless decimal string on the JSON wire.
#[test]
fn point_lookup_row_id_round_trips_full_u128() {
    let row_id = RowId((7u128 << 64) | u128::from(u64::MAX));
    let plan = PhysicalPlan::PointLookup {
        row_id,
        columns: Vec::new(),
    };
    let json = serde_json::to_string(&plan).unwrap();

    assert!(json.contains("\"row_id\":\""));
    let decoded: PhysicalPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, plan);
}

/// The small numeric row-id form of the original 2.0 wire is still accepted.
#[test]
fn point_lookup_accepts_legacy_numeric_row_id() {
    let decoded: PhysicalPlan =
        serde_json::from_str(r#"{"op":"point_lookup","row_id":1,"columns":[]}"#).unwrap();
    match decoded {
        PhysicalPlan::PointLookup { row_id, .. } => assert_eq!(row_id, RowId(1)),
        _ => panic!("expected point lookup"),
    }
}

/// An entity scan carries its field-to-slot mapping; slots are plain JSON numbers.
#[test]
fn entity_scan_round_trips_through_json() {
    let json = r#"{"op":"entity_scan","entity_id":42,"columns":[{"field_id":1,"slot":5}]}"#;
    let plan: PhysicalPlan = serde_json::from_str(json).unwrap();
    assert_eq!(
        plan,
        PhysicalPlan::EntityScan {
            entity_id: 42,
            columns: vec![ScanColumn {
                field_id: 1,
                slot: SlotId(5)
            }],
        }
    );
    assert_eq!(plan.validate().unwrap().width, 6);
    assert_eq!(serde_json::to_string(&plan).unwrap(), json);
}
