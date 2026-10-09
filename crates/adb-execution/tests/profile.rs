//! Per-operator runtime profiles (the data behind EXPLAIN ANALYZE).
mod support;

use std::sync::Arc;

use adb_core::{CommitTs, RowId};
use adb_execution::{
    AggregateFunction, AggregateSpec, BinaryOp, ExecutionContext, Executor, Expr, JoinKey,
    JoinType, OperatorProfile, PhysicalPlan, SortKey,
};
use support::*;

/// 20 rows of entity 1 (id, group = id % 4) and 4 rows of entity 2 (group, label).
fn source() -> MemorySource {
    let facts = (0..20u64).map(|id| {
        (
            RowId::compose(1, id),
            row(&[(1, i(id as i64)), (2, i((id % 4) as i64))]),
        )
    });
    let dims = (0..4u64).map(|group| {
        (
            RowId::compose(2, group),
            row(&[(1, i(group as i64)), (2, t(&format!("g{group}")))]),
        )
    });
    MemorySource::new(facts.chain(dims).collect())
}

/// `TopK(Aggregate(HashJoin(Filter(scan 1), scan 2)))`.
fn plan() -> PhysicalPlan {
    PhysicalPlan::TopK {
        input: Box::new(PhysicalPlan::Aggregate {
            input: Box::new(PhysicalPlan::HashJoin {
                left: Box::new(PhysicalPlan::Filter {
                    input: Box::new(scan(1, &[(1, 0), (2, 1)])),
                    predicate: Expr::Binary {
                        left: Box::new(Expr::Slot { slot: s(0) }),
                        op: BinaryOp::Lt,
                        right: Box::new(Expr::Literal { value: i(10) }),
                    },
                }),
                right: Box::new(scan(2, &[(1, 2), (2, 3)])),
                join_type: JoinType::Inner,
                keys: vec![JoinKey {
                    left: s(1),
                    right: s(2),
                }],
                residual: None,
            }),
            group_by: vec![s(3)],
            aggregates: vec![AggregateSpec {
                function: AggregateFunction::Count,
                input: None,
                output: s(4),
            }],
        }),
        keys: vec![SortKey {
            slot: s(4),
            descending: true,
        }],
        limit: 2,
    }
}

/// Counter `name` of `profile`.
fn counter(profile: &OperatorProfile, name: &str) -> u64 {
    profile.counters[name]
}

/// The profile mirrors the plan tree and reports rows, selectivity, build sizes and memory.
#[test]
fn profile_reports_every_operator() {
    let mut cursor = Executor::execute(
        Arc::new(source()),
        plan(),
        ExecutionContext::new(CommitTs(1)),
    )
    .unwrap();
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch().unwrap() {
        rows += batch.len();
    }
    assert_eq!(rows, 2);

    let profile = cursor.profile();
    let top_k = &profile.root;
    assert_eq!(top_k.operator, "top_k");
    assert_eq!(top_k.rows_out, 2);
    assert_eq!(counter(top_k, "rows_in"), 4);

    let aggregate = &top_k.children[0];
    assert_eq!(aggregate.operator, "aggregate");
    assert_eq!(counter(aggregate, "groups"), 4);
    assert_eq!(counter(aggregate, "rows_in"), 10);

    let join = &aggregate.children[0];
    assert_eq!(join.operator, "hash_join");
    assert_eq!(join.rows_out, 10);
    assert_eq!(counter(join, "build_rows"), 4);
    assert_eq!(counter(join, "build_keys"), 4);
    assert_eq!(counter(join, "probe_rows"), 10);
    assert!(counter(join, "peak_memory_bytes") > 0);

    let filter = &join.children[0];
    assert_eq!(filter.operator, "filter");
    assert_eq!(counter(filter, "rows_in"), 20);
    assert_eq!(filter.rows_out, 10);
    assert_eq!(filter.children[0].operator, "entity_scan");
    assert_eq!(join.children[1].operator, "entity_scan");
    assert_eq!(join.children[1].rows_out, 4);

    assert!(profile.peak_memory_bytes > 0);
    assert_eq!(cursor.metrics().source_rows, 24);
}

/// Operators are numbered in plan pre-order: TopK 0, Aggregate 1, HashJoin 2, Filter 3,
/// probe scan 4, build scan 5.
#[test]
fn profile_numbers_nodes_in_plan_preorder() {
    let plan = plan();
    let nodes = plan.node_count();
    let mut cursor =
        Executor::execute(Arc::new(source()), plan, ExecutionContext::new(CommitTs(1))).unwrap();
    while cursor.next_batch().unwrap().is_some() {}
    let profile = cursor.profile();

    /// Collects `(node_id, operator)` in pre-order.
    fn walk(profile: &OperatorProfile, out: &mut Vec<(u32, &'static str)>) {
        out.push((profile.node_id, profile.operator));
        profile.children.iter().for_each(|child| walk(child, out));
    }
    let mut ids = Vec::new();
    walk(&profile.root, &mut ids);
    assert_eq!(
        ids,
        vec![
            (0, "top_k"),
            (1, "aggregate"),
            (2, "hash_join"),
            (3, "filter"),
            (4, "entity_scan"),
            (5, "entity_scan"),
        ]
    );
    assert_eq!(nodes, ids.len());
    let json = serde_json::to_value(&profile).unwrap();
    assert_eq!(json["root"]["children"][0]["children"][0]["node_id"], 2);
}

/// The profile serializes to the JSON shape returned through the C ABI.
#[test]
fn profile_serializes_to_json() {
    let mut cursor = Executor::execute(
        Arc::new(source()),
        plan(),
        ExecutionContext::new(CommitTs(1)),
    )
    .unwrap();
    while cursor.next_batch().unwrap().is_some() {}
    let json = serde_json::to_value(cursor.profile()).unwrap();
    assert_eq!(json["root"]["operator"], "top_k");
    assert_eq!(
        json["root"]["children"][0]["children"][0]["counters"]["build_rows"],
        4
    );
    assert!(json["peak_memory_bytes"].as_u64().unwrap() > 0);
}
