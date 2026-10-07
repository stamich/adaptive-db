//! Plan wire v2: envelope, version negotiation, strictness and validation.
use adb_core::{RowId, Value};
use adb_execution::{
    AggregateFunction, AggregateSpec, BinaryOp, Expr, JoinKey, JoinType, PhysicalPlan, ScanColumn,
    SlotId, SortKey,
};
use adb_plan_wire::{decode_json, encode_json, PlanWireError, MAX_PLAN_WIRE_BYTES};

/// Scan of `entity` reading field 1 into `slot`.
fn scan(entity: u64, slot: u32) -> PhysicalPlan {
    PhysicalPlan::EntityScan {
        entity_id: entity,
        columns: vec![ScanColumn {
            field_id: 1,
            slot: SlotId(slot),
        }],
    }
}

/// A plan using every 2.1 node type.
fn relational_plan() -> PhysicalPlan {
    PhysicalPlan::TopK {
        input: Box::new(PhysicalPlan::Aggregate {
            input: Box::new(PhysicalPlan::NestedLoopJoin {
                left: Box::new(PhysicalPlan::HashJoin {
                    left: Box::new(scan(1, 0)),
                    right: Box::new(scan(2, 1)),
                    join_type: JoinType::Left,
                    keys: vec![JoinKey {
                        left: SlotId(0),
                        right: SlotId(1),
                    }],
                    residual: None,
                }),
                right: Box::new(PhysicalPlan::PointLookup {
                    row_id: RowId(u128::MAX),
                    columns: vec![ScanColumn {
                        field_id: 2,
                        slot: SlotId(2),
                    }],
                }),
                join_type: JoinType::Inner,
                predicate: Some(Expr::Binary {
                    left: Box::new(Expr::Slot { slot: SlotId(0) }),
                    op: BinaryOp::Lt,
                    right: Box::new(Expr::Literal {
                        value: Value::Int64(5),
                    }),
                }),
            }),
            group_by: vec![SlotId(2)],
            aggregates: vec![AggregateSpec {
                function: AggregateFunction::Avg,
                input: Some(SlotId(0)),
                output: SlotId(3),
            }],
        }),
        keys: vec![SortKey {
            slot: SlotId(3),
            descending: true,
        }],
        limit: 10,
    }
}

/// Every node type survives an encode/decode round trip, and decoding reports the shape.
#[test]
fn relational_plan_round_trips() {
    let plan = relational_plan();
    let bytes = encode_json(&plan).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.starts_with(r#"{"wire_version":2,"plan":{"op":"top_k""#));
    let (decoded, shape) = decode_json(&bytes).unwrap();
    assert_eq!(decoded, plan);
    assert_eq!(shape.output, vec![SlotId(2), SlotId(3)]);
    assert_eq!(shape.width, 4);
}

/// A bare 2.0 plan (no envelope) and a future version are rejected as version mismatches.
#[test]
fn version_mismatch_is_reported_as_such() {
    assert_eq!(
        decode_json(br#"{"op":"entity_scan","entity_id":7}"#),
        Err(PlanWireError::UnsupportedVersion { found: 1 })
    );
    assert_eq!(
        decode_json(br#"{"wire_version":3,"plan":{"op":"something_new"}}"#),
        Err(PlanWireError::UnsupportedVersion { found: 3 })
    );
}

/// Unknown fields are errors, so a producer speaking another dialect cannot be half-understood.
#[test]
fn unknown_fields_are_rejected() {
    let stale = br#"{"wire_version":2,"plan":{"op":"project","input":{"op":"entity_scan","entity_id":1,"columns":[]},"fields":[1]}}"#;
    assert!(matches!(
        decode_json(stale),
        Err(PlanWireError::Malformed(_))
    ));
    let extra = br#"{"wire_version":2,"plan":{"op":"scan","columns":[]},"hint":"x"}"#;
    assert!(matches!(
        decode_json(extra),
        Err(PlanWireError::Malformed(_))
    ));
}

/// Decoded plans are validated; encoding refuses invalid plans too.
#[test]
fn invalid_plans_are_rejected_both_ways() {
    let bad = br#"{"wire_version":2,"plan":{"op":"project","input":{"op":"scan","columns":[]},"slots":[4]}}"#;
    assert!(matches!(decode_json(bad), Err(PlanWireError::Invalid(_))));

    let invalid = PhysicalPlan::Limit {
        input: Box::new(scan(1, 0)),
        limit: usize::MAX,
    };
    assert!(matches!(
        encode_json(&invalid),
        Err(PlanWireError::Invalid(_))
    ));
}

/// Documents are size-bounded.
#[test]
fn size_is_bounded() {
    assert_eq!(decode_json(b""), Err(PlanWireError::Size(0)));
    let huge = vec![b' '; MAX_PLAN_WIRE_BYTES + 1];
    assert_eq!(
        decode_json(&huge),
        Err(PlanWireError::Size(MAX_PLAN_WIRE_BYTES + 1))
    );
}
