//! Structural validation of physical plans: limits and slot consistency.
mod support;

use adb_execution::{
    limits::{MAX_LIMIT, MAX_LIST_LEN, MAX_PLAN_DEPTH, MAX_SLOTS},
    BinaryOp, Expr, PhysicalPlan,
};
use support::*;

/// Error message of an invalid plan.
fn invalid(plan: &PhysicalPlan) -> String {
    plan.validate().expect_err("plan must be rejected")
}

/// A valid plan reports its row width and output slots.
#[test]
fn valid_plan_reports_shape() {
    let plan = PhysicalPlan::Project {
        input: Box::new(scan(1, &[(1, 0), (2, 4)])),
        slots: vec![s(4)],
    };
    let shape = plan.validate().unwrap();
    assert_eq!(shape.width, 5);
    assert_eq!(shape.output, vec![s(4)]);
}

/// Reading a slot the input does not produce is rejected.
#[test]
fn unknown_slot_is_rejected() {
    let plan = PhysicalPlan::Filter {
        input: Box::new(scan(1, &[(1, 0)])),
        predicate: Expr::Binary {
            left: Box::new(Expr::Slot { slot: s(9) }),
            op: BinaryOp::Eq,
            right: Box::new(Expr::Slot { slot: s(0) }),
        },
    };
    assert!(invalid(&plan).contains("#9"));

    let project = PhysicalPlan::Project {
        input: Box::new(scan(1, &[(1, 0)])),
        slots: vec![s(1)],
    };
    assert!(invalid(&project).contains("does not produce"));
}

/// Two columns may not be written to the same slot.
#[test]
fn duplicate_slot_is_rejected() {
    assert!(invalid(&scan(1, &[(1, 0), (2, 0)])).contains("twice"));
}

/// Slot ids, lists, LIMIT and depth are bounded.
#[test]
fn limits_are_enforced() {
    assert!(invalid(&scan(1, &[(1, MAX_SLOTS)])).contains("slot"));

    let wide: Vec<(u32, u32)> = (0..=MAX_LIST_LEN as u32).map(|n| (n, n)).collect();
    assert!(invalid(&scan(1, &wide)).contains("exceed"));

    let limit = PhysicalPlan::Limit {
        input: Box::new(scan(1, &[])),
        limit: MAX_LIMIT + 1,
    };
    assert!(invalid(&limit).contains("LIMIT"));

    let mut deep = scan(1, &[]);
    for _ in 0..=MAX_PLAN_DEPTH {
        deep = PhysicalPlan::Limit {
            input: Box::new(deep),
            limit: 1,
        };
    }
    assert!(invalid(&deep).contains("depth"));
}

/// Index bounds that no index can accept are rejected before execution: more key columns than
/// the 16 an index may have, or a NULL bound.
#[test]
fn impossible_index_bounds_are_rejected() {
    use adb_core::Value;
    use adb_index::{IndexBounds, ScanDirection, ValueBound};
    let scan = |bounds: IndexBounds| PhysicalPlan::IndexScan {
        index_id: 1,
        bounds,
        direction: ScanDirection::Forward,
        columns: Vec::new(),
        limit: None,
    };
    let sixteen = IndexBounds {
        prefix: (0..16).map(Value::Int64).collect(),
        ..IndexBounds::all()
    };
    assert!(scan(sixteen.clone()).validate().is_ok());
    let ranged = IndexBounds {
        lower: Some(ValueBound {
            value: Value::Int64(1),
            inclusive: true,
        }),
        ..sixteen
    };
    assert!(invalid(&scan(ranged)).contains("at most 16"));
    let null = IndexBounds {
        upper: Some(ValueBound {
            value: Value::Null,
            inclusive: false,
        }),
        ..IndexBounds::all()
    };
    assert!(invalid(&scan(null)).contains("NULL"));
}
