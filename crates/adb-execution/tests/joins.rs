//! HashJoin and NestedLoopJoin: semantics, slot handling and hardening limits.
mod support;

use adb_core::{CommitTs, RowId, Value};
use adb_execution::{
    BinaryOp, ExecutionContext, ExecutionError, ExecutionLimits, Expr, JoinKey, JoinType,
    PhysicalPlan,
};
use support::*;

/// Entity 1 = customers(id: f1, name: f2); entity 2 = orders(id: f1, customer_id: f2, amount: f3).
///
/// Customer 3 has no orders; order 14 references a missing customer; order 15 has a NULL
/// customer id.
fn shop() -> MemorySource {
    let customers = [(1, "ann"), (2, "bob"), (3, "cid")].map(|(id, name)| {
        (
            RowId::compose(1, id),
            row(&[(1, i(id as i64)), (2, t(name))]),
        )
    });
    let orders = [
        (10, Some(1), 100),
        (11, Some(1), 250),
        (12, Some(2), 40),
        (13, Some(2), 60),
        (14, Some(9), 5),
        (15, None, 7),
    ]
    .map(|(id, customer, amount)| {
        let customer = customer.map_or(Value::Null, |c: i64| i(c));
        (
            RowId::compose(2, id),
            row(&[(1, i(id as i64)), (2, customer), (3, i(amount))]),
        )
    });
    MemorySource::new(customers.into_iter().chain(orders).collect())
}

/// customers -> slots 0 (id), 1 (name).
fn customers() -> PhysicalPlan {
    scan(1, &[(1, 0), (2, 1)])
}

/// orders -> slots 2 (id), 3 (customer_id), 4 (amount).
fn orders() -> PhysicalPlan {
    scan(2, &[(1, 2), (2, 3), (3, 4)])
}

/// `customers JOIN orders ON c.id = o.customer_id` as a hash join.
fn hash_join(join_type: JoinType, residual: Option<Expr>) -> PhysicalPlan {
    PhysicalPlan::HashJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type,
        keys: vec![JoinKey {
            left: s(0),
            right: s(3),
        }],
        residual,
    }
}

/// `left.slot op right.slot`.
fn compare(left: u32, op: BinaryOp, right: u32) -> Expr {
    Expr::Binary {
        left: Box::new(Expr::Slot { slot: s(left) }),
        op,
        right: Box::new(Expr::Slot { slot: s(right) }),
    }
}

/// `slot op literal`.
fn compare_literal(slot: u32, op: BinaryOp, value: i64) -> Expr {
    Expr::Binary {
        left: Box::new(Expr::Slot { slot: s(slot) }),
        op,
        right: Box::new(Expr::Literal { value: i(value) }),
    }
}

/// Output rows as `[name, order id]`, sorted for order-independent comparison.
fn name_and_order(plan: PhysicalPlan) -> Vec<Vec<Value>> {
    let mut rows = rows_of(shop(), plan, &[1, 2]);
    rows.sort_by_key(|row| format!("{row:?}"));
    rows
}

/// INNER hash join returns exactly the matching pairs; NULL and dangling keys never match.
#[test]
fn inner_hash_join_matches_equal_keys() {
    assert_eq!(
        name_and_order(hash_join(JoinType::Inner, None)),
        vec![
            vec![t("ann"), i(10)],
            vec![t("ann"), i(11)],
            vec![t("bob"), i(12)],
            vec![t("bob"), i(13)],
        ]
    );
}

/// LEFT hash join keeps unmatched left rows once, with NULL right slots, and no row id.
#[test]
fn left_hash_join_null_fills_unmatched_rows() {
    let rows = name_and_order(hash_join(JoinType::Left, None));
    assert_eq!(rows.len(), 5);
    assert!(rows.contains(&vec![t("cid"), Value::Null]));

    let batches = run_with(
        shop(),
        hash_join(JoinType::Left, None),
        ExecutionContext::new(CommitTs(1)),
    )
    .unwrap();
    assert!(batches.iter().all(|batch| batch.row_ids.is_none()));
}

/// A residual is part of the join condition: under LEFT JOIN a left row whose matches all fail
/// the residual is null-filled instead of disappearing.
#[test]
fn residual_condition_decides_left_join_matches() {
    let big_orders = compare_literal(4, BinaryOp::Gt, 50);
    assert_eq!(
        name_and_order(hash_join(JoinType::Inner, Some(big_orders.clone()))),
        vec![
            vec![t("ann"), i(10)],
            vec![t("ann"), i(11)],
            vec![t("bob"), i(13)],
        ]
    );

    let only_huge = compare_literal(4, BinaryOp::Gt, 200);
    assert_eq!(
        name_and_order(hash_join(JoinType::Left, Some(only_huge))),
        vec![
            vec![t("ann"), i(11)],
            vec![t("bob"), Value::Null],
            vec![t("cid"), Value::Null],
        ]
    );
}

/// Two scans of the same entity use different slots, so a self-join keeps both sides apart.
#[test]
fn self_join_keeps_relation_instances_apart() {
    // orders a JOIN orders b ON a.customer_id = b.customer_id AND a.id < b.id
    let plan = PhysicalPlan::HashJoin {
        left: Box::new(scan(2, &[(1, 0), (2, 1)])),
        right: Box::new(scan(2, &[(1, 10), (2, 11)])),
        join_type: JoinType::Inner,
        keys: vec![JoinKey {
            left: s(1),
            right: s(11),
        }],
        residual: Some(compare(0, BinaryOp::Lt, 10)),
    };
    let rows = rows_of(shop(), plan, &[0, 10]);
    assert_eq!(rows, vec![vec![i(10), i(11)], vec![i(12), i(13)]]);
}

/// Multi-column keys require every component to be equal.
#[test]
fn composite_keys_compare_every_component() {
    // customers c JOIN orders o ON c.id = o.customer_id AND c.id = o.id (never true)
    let plan = PhysicalPlan::HashJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        keys: vec![
            JoinKey {
                left: s(0),
                right: s(3),
            },
            JoinKey {
                left: s(0),
                right: s(2),
            },
        ],
        residual: None,
    };
    assert!(rows_of(shop(), plan, &[0]).is_empty());
}

/// The nested-loop join handles non-equality conditions and gives the same answer as the hash
/// join for an equality condition.
#[test]
fn nested_loop_join_matches_hash_join_and_handles_inequalities() {
    let equi = PhysicalPlan::NestedLoopJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Left,
        predicate: Some(compare(0, BinaryOp::Eq, 3)),
    };
    assert_eq!(
        name_and_order(equi),
        name_and_order(hash_join(JoinType::Left, None))
    );

    // customers c JOIN orders o ON o.customer_id > c.id
    let inequality = PhysicalPlan::NestedLoopJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        predicate: Some(compare(3, BinaryOp::Gt, 0)),
    };
    let rows = rows_of(shop(), inequality, &[0, 2]);
    // ann(1) < bob's 12, 13 and dangling 14; bob(2) < 14; cid(3) < 14.
    assert_eq!(rows.len(), 5);

    let cross = PhysicalPlan::NestedLoopJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        predicate: None,
    };
    assert_eq!(rows_of(shop(), cross, &[0]).len(), 18);
}

/// Context with one runtime limit changed.
fn limited(change: impl FnOnce(&mut ExecutionLimits)) -> ExecutionContext {
    let mut limits = ExecutionLimits::default();
    change(&mut limits);
    ExecutionContext::new(CommitTs(1)).with_limits(limits)
}

/// Asserts that running `plan` under `context` fails with `ResourceLimit` mentioning `what`.
fn assert_resource_limit(plan: PhysicalPlan, context: ExecutionContext, what: &str) {
    match run_with(shop(), plan, context) {
        Err(ExecutionError::ResourceLimit(message)) => {
            assert!(message.contains(what), "unexpected message: {message}")
        }
        other => panic!("expected a resource limit error, got {other:?}"),
    }
}

/// One left row may not produce more matches than the fanout limit.
#[test]
fn join_fanout_is_capped() {
    assert_resource_limit(
        hash_join(JoinType::Inner, None),
        limited(|limits| limits.max_join_fanout = 1),
        "fanout",
    );
}

/// The nested-loop join stops at the comparison budget.
#[test]
fn nested_loop_comparisons_are_capped() {
    let plan = PhysicalPlan::NestedLoopJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        predicate: None,
    };
    assert_resource_limit(
        plan,
        limited(|limits| limits.max_nested_loop_comparisons = 10),
        "comparisons",
    );
}

/// The build side is memory-accounted and row-capped, and the memory is returned afterwards.
#[test]
fn build_side_respects_memory_and_row_limits() {
    assert_resource_limit(
        hash_join(JoinType::Inner, None),
        limited(|limits| limits.query_memory_bytes = 256),
        "memory",
    );
    assert_resource_limit(
        hash_join(JoinType::Inner, None),
        limited(|limits| limits.max_materialized_rows = 3),
        "rows",
    );

    let context = ExecutionContext::new(CommitTs(1));
    let memory = context.memory.clone();
    run_with(shop(), hash_join(JoinType::Inner, None), context).unwrap();
    assert!(memory.peak() > 0);
    assert_eq!(
        memory.used(),
        0,
        "the build side is released with the cursor"
    );
}

/// Output batches stay near the batch size even when one left row has many matches.
#[test]
fn join_output_is_batched() {
    let mut context = ExecutionContext::new(CommitTs(1));
    context.batch_size = 2;
    let cross = PhysicalPlan::NestedLoopJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        predicate: None,
    };
    let batches = run_with(shop(), cross, context).unwrap();
    assert_eq!(batches.iter().map(|batch| batch.len()).sum::<usize>(), 18);
    // batch_size (2) + one left row's matches (6) is the upper bound
    assert!(batches.iter().all(|batch| batch.len() <= 2 + 6));
    assert!(batches.len() >= 3);
}

/// Join plans with overlapping or misplaced slots are rejected before execution.
#[test]
fn invalid_join_plans_are_rejected() {
    let overlapping = PhysicalPlan::NestedLoopJoin {
        left: Box::new(scan(1, &[(1, 0)])),
        right: Box::new(scan(2, &[(1, 0)])),
        join_type: JoinType::Inner,
        predicate: None,
    };
    assert!(overlapping.validate().unwrap_err().contains("twice"));

    let wrong_side = PhysicalPlan::HashJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        keys: vec![JoinKey {
            left: s(3),
            right: s(0),
        }],
        residual: None,
    };
    assert!(wrong_side.validate().unwrap_err().contains("left key"));

    let no_keys = PhysicalPlan::HashJoin {
        left: Box::new(customers()),
        right: Box::new(orders()),
        join_type: JoinType::Inner,
        keys: Vec::new(),
        residual: None,
    };
    assert!(no_keys.validate().is_err());
}
