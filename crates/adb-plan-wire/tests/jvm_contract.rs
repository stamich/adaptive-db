//! Cross-language contract: a plan encoded by the JVM planner decodes and validates natively.
//!
//! `tests/fixtures/jvm-relational-plan.json` is the exact output of the Scala `PlanJsonEncoder`
//! for `PlanningFixtures.ContractQuery`; the Scala test `matchesTheNativeContractFixture`
//! asserts the encoder still produces these bytes. Together the two tests pin the wire format
//! from both sides.
use adb_execution::{JoinType, PhysicalPlan, SlotId};
use adb_plan_wire::decode_json;

/// The JVM fixture decodes, validates, and has the expected relational shape.
#[test]
fn jvm_encoded_plan_decodes_and_validates() {
    let bytes = include_bytes!("fixtures/jvm-relational-plan.json");
    let (plan, shape) = decode_json(bytes.trim_ascii_end()).expect("JVM plan must decode");

    // SELECT c.name, SUM(o.amount) AS total: two output columns.
    assert_eq!(shape.output.len(), 2);
    let PhysicalPlan::Project { input, .. } = &plan else {
        panic!("expected a projection at the root, got {plan:?}");
    };
    let PhysicalPlan::TopK { input, limit, keys } = input.as_ref() else {
        panic!("expected TopK under the projection");
    };
    assert_eq!(*limit, 3);
    assert!(keys[0].descending);
    let PhysicalPlan::Aggregate { input, .. } = input.as_ref() else {
        panic!("expected an aggregate under TopK");
    };
    let PhysicalPlan::HashJoin {
        join_type, left, ..
    } = input.as_ref()
    else {
        panic!("expected the LEFT hash join");
    };
    assert_eq!(*join_type, JoinType::Left);
    assert!(matches!(
        left.as_ref(),
        PhysicalPlan::HashJoin {
            join_type: JoinType::Inner,
            ..
        }
    ));
    // Non-ASCII literals survive the trip.
    assert!(std::str::from_utf8(bytes).unwrap().contains("Kraków"));
    assert!(shape.width > shape.output.iter().map(|s: &SlotId| s.index()).max().unwrap());
}
