//! Aggregate, Sort and TopK: semantics, checked arithmetic and limits.
mod support;

use std::sync::atomic::Ordering;

use adb_core::{CommitTs, RowId, Value};
use adb_execution::{
    AggregateFunction, AggregateSpec, ExecutionContext, ExecutionError, ExecutionLimits, JoinKey,
    JoinType, PhysicalPlan, SortKey,
};
use support::*;

/// Entity 2 = sales(id: f1, region: f2, amount: f3, rep: f4); one row has a NULL region and
/// one a NULL amount.
fn sales() -> MemorySource {
    let rows = [
        (1, Some("north"), Some(10), "kim"),
        (2, Some("south"), Some(5), "lee"),
        (3, Some("north"), Some(30), "abe"),
        (4, None, Some(7), "zed"),
        (5, Some("south"), None, "max"),
        (6, Some("north"), Some(20), "bea"),
    ];
    MemorySource::new(
        rows.into_iter()
            .map(|(id, region, amount, rep)| {
                (
                    RowId::compose(2, id),
                    row(&[
                        (1, i(id as i64)),
                        (2, region.map_or(Value::Null, t)),
                        (3, amount.map_or(Value::Null, i)),
                        (4, t(rep)),
                    ]),
                )
            })
            .collect(),
    )
}

/// sales -> slots 0 (id), 1 (region), 2 (amount), 3 (rep).
fn sales_scan() -> PhysicalPlan {
    scan(2, &[(1, 0), (2, 1), (3, 2), (4, 3)])
}

/// An aggregate spec.
fn agg(function: AggregateFunction, input: Option<u32>, output: u32) -> AggregateSpec {
    AggregateSpec {
        function,
        input: input.map(s),
        output: s(output),
    }
}

/// A sort key.
fn key(slot: u32, descending: bool) -> SortKey {
    SortKey {
        slot: s(slot),
        descending,
    }
}

/// GROUP BY computes every aggregate per group; NULL keys form one group; NULL inputs are
/// skipped by everything but COUNT(*).
#[test]
fn group_by_computes_all_aggregates() {
    let plan = PhysicalPlan::Sort {
        input: Box::new(PhysicalPlan::Aggregate {
            input: Box::new(sales_scan()),
            group_by: vec![s(1)],
            aggregates: vec![
                agg(AggregateFunction::Count, None, 10),
                agg(AggregateFunction::Count, Some(2), 11),
                agg(AggregateFunction::Sum, Some(2), 12),
                agg(AggregateFunction::Min, Some(3), 13),
                agg(AggregateFunction::Max, Some(2), 14),
                agg(AggregateFunction::Avg, Some(2), 15),
            ],
        }),
        keys: vec![key(1, false)],
    };
    let rows = rows_of(sales(), plan, &[1, 10, 11, 12, 13, 14, 15]);
    assert_eq!(
        rows,
        vec![
            vec![
                t("north"),
                i(3),
                i(3),
                i(60),
                t("abe"),
                i(30),
                Value::Float64(20.0)
            ],
            vec![
                t("south"),
                i(2),
                i(1),
                i(5),
                t("lee"),
                i(5),
                Value::Float64(5.0)
            ],
            vec![
                Value::Null,
                i(1),
                i(1),
                i(7),
                t("zed"),
                i(7),
                Value::Float64(7.0)
            ],
        ]
    );
}

/// Without GROUP BY there is always exactly one row; with GROUP BY, empty input gives none.
#[test]
fn empty_input_aggregates() {
    let empty = || PhysicalPlan::Limit {
        input: Box::new(sales_scan()),
        limit: 0,
    };
    let global = PhysicalPlan::Aggregate {
        input: Box::new(empty()),
        group_by: Vec::new(),
        aggregates: vec![
            agg(AggregateFunction::Count, None, 10),
            agg(AggregateFunction::Sum, Some(2), 11),
        ],
    };
    assert_eq!(
        rows_of(sales(), global, &[10, 11]),
        vec![vec![i(0), Value::Null]]
    );

    let grouped = PhysicalPlan::Aggregate {
        input: Box::new(empty()),
        group_by: vec![s(1)],
        aggregates: vec![agg(AggregateFunction::Count, None, 10)],
    };
    assert!(rows_of(sales(), grouped, &[10]).is_empty());
}

/// Source with the given INT64 values in field 1 of entity 3.
fn numbers(values: &[i64]) -> MemorySource {
    MemorySource::new(
        values
            .iter()
            .enumerate()
            .map(|(n, value)| (RowId::compose(3, n as u64), row(&[(1, i(*value))])))
            .collect(),
    )
}

/// `SUM` over field 1 of entity 3.
fn sum_plan() -> PhysicalPlan {
    PhysicalPlan::Aggregate {
        input: Box::new(scan(3, &[(1, 0)])),
        group_by: Vec::new(),
        aggregates: vec![agg(AggregateFunction::Sum, Some(0), 1)],
    }
}

/// INT64 SUM is exact: intermediate sums may leave the INT64 range, but a final result that
/// does not fit is an `ArithmeticOverflow` error, never a wrapped value.
#[test]
fn sum_is_checked() {
    assert_eq!(
        rows_of(numbers(&[i64::MAX, 1, -2]), sum_plan(), &[1]),
        vec![vec![i(i64::MAX - 1)]]
    );
    let overflow = run_with(
        numbers(&[i64::MAX, 1]),
        sum_plan(),
        ExecutionContext::new(CommitTs(1)),
    );
    assert!(matches!(
        overflow,
        Err(ExecutionError::ArithmeticOverflow(_))
    ));
}

/// Mixed value types in one aggregate input are rejected.
#[test]
fn mixed_types_are_rejected() {
    let source = MemorySource::new(vec![
        (RowId::compose(3, 1), row(&[(1, i(1))])),
        (RowId::compose(3, 2), row(&[(1, Value::Float64(2.5))])),
    ]);
    let result = run_with(source, sum_plan(), ExecutionContext::new(CommitTs(1)));
    assert!(matches!(result, Err(ExecutionError::Expression(_))));
}

/// The number of groups is capped.
#[test]
fn group_count_is_capped() {
    let limits = ExecutionLimits {
        max_materialized_rows: 2,
        ..ExecutionLimits::default()
    };
    let plan = PhysicalPlan::Aggregate {
        input: Box::new(sales_scan()),
        group_by: vec![s(1)],
        aggregates: vec![agg(AggregateFunction::Count, None, 10)],
    };
    let result = run_with(
        sales(),
        plan,
        ExecutionContext::new(CommitTs(1)).with_limits(limits),
    );
    assert!(matches!(result, Err(ExecutionError::ResourceLimit(_))));
}

/// Aggregate validation: outputs must be new slots and SUM needs an input.
#[test]
fn invalid_aggregates_are_rejected() {
    let clash = PhysicalPlan::Aggregate {
        input: Box::new(sales_scan()),
        group_by: vec![s(1)],
        aggregates: vec![agg(AggregateFunction::Count, None, 2)],
    };
    assert!(clash.validate().unwrap_err().contains("already produced"));

    let no_input = PhysicalPlan::Aggregate {
        input: Box::new(sales_scan()),
        group_by: Vec::new(),
        aggregates: vec![agg(AggregateFunction::Sum, None, 10)],
    };
    assert!(no_input.validate().unwrap_err().contains("input slot"));
}

/// Multi-key sort: region ascending with NULL last, then amount descending with NULL first.
#[test]
fn sort_orders_by_multiple_keys_with_null_placement() {
    let plan = PhysicalPlan::Sort {
        input: Box::new(sales_scan()),
        keys: vec![key(1, false), key(2, true)],
    };
    let ids: Vec<Value> = rows_of(sales(), plan, &[0])
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    assert_eq!(ids, vec![i(3), i(6), i(1), i(5), i(2), i(4)]);
}

/// Sorting values that cannot be ordered fails cleanly instead of panicking.
#[test]
fn unsortable_values_are_errors() {
    let source = MemorySource::new(vec![
        (RowId::compose(3, 1), row(&[(1, Value::Float64(1.0))])),
        (RowId::compose(3, 2), row(&[(1, Value::Float64(f64::NAN))])),
    ]);
    let plan = PhysicalPlan::Sort {
        input: Box::new(scan(3, &[(1, 0)])),
        keys: vec![key(0, false)],
    };
    let result = run_with(source, plan, ExecutionContext::new(CommitTs(1)));
    assert!(matches!(result, Err(ExecutionError::Expression(_))));
}

/// Deterministic pseudo-random rows with many duplicate keys.
fn noisy(count: u64) -> MemorySource {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    MemorySource::new(
        (0..count)
            .map(|n| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let value = if state.is_multiple_of(17) {
                    Value::Null
                } else {
                    i((state % 50) as i64)
                };
                (RowId::compose(4, n), row(&[(1, value), (2, i(n as i64))]))
            })
            .collect(),
    )
}

/// TopK returns exactly what `Limit(Sort)` returns, including tie order and NULL placement,
/// for k smaller than, equal to and larger than the input; it holds less memory than Sort.
#[test]
fn top_k_equals_limit_over_sort() {
    let keys = vec![key(0, true)];
    let input = || Box::new(scan(4, &[(1, 0), (2, 1)]));
    for k in [1, 7, 100, 999, 1000, 5000] {
        let top_k = PhysicalPlan::TopK {
            input: input(),
            keys: keys.clone(),
            limit: k,
        };
        let limit_sort = PhysicalPlan::Limit {
            input: Box::new(PhysicalPlan::Sort {
                input: input(),
                keys: keys.clone(),
            }),
            limit: k,
        };
        assert_eq!(
            rows_of(noisy(1000), top_k, &[0, 1]),
            rows_of(noisy(1000), limit_sort, &[0, 1]),
            "k = {k}"
        );
    }

    let peak = |plan: PhysicalPlan| {
        let context = ExecutionContext::new(CommitTs(1));
        let memory = context.memory.clone();
        run_with(noisy(1000), plan, context).unwrap();
        memory.peak()
    };
    let top_peak = peak(PhysicalPlan::TopK {
        input: input(),
        keys: keys.clone(),
        limit: 10,
    });
    let sort_peak = peak(PhysicalPlan::Sort {
        input: input(),
        keys,
    });
    assert!(
        top_peak * 10 < sort_peak,
        "top-k {top_peak} vs sort {sort_peak}"
    );
}

/// TopK respects the materialized-row cap: k above it is refused, and the buffer stays within it
/// while still producing the exact result.
#[test]
fn top_k_respects_the_row_cap() {
    let limits = |rows| ExecutionLimits {
        max_materialized_rows: rows,
        ..ExecutionLimits::default()
    };
    let plan = |k| PhysicalPlan::TopK {
        input: Box::new(scan(4, &[(1, 0), (2, 1)])),
        keys: vec![key(0, true)],
        limit: k,
    };
    let refused = run_with(
        noisy(100),
        plan(11),
        ExecutionContext::new(CommitTs(1)).with_limits(limits(10)),
    );
    assert!(matches!(refused, Err(ExecutionError::ResourceLimit(_))));

    let capped = run_with(
        noisy(1000),
        plan(10),
        ExecutionContext::new(CommitTs(1)).with_limits(limits(10)),
    )
    .unwrap();
    let rows: usize = capped.iter().map(|batch| batch.len()).sum();
    assert_eq!(rows, 10);
    let expected = rows_of(
        noisy(1000),
        PhysicalPlan::Limit {
            input: Box::new(PhysicalPlan::Sort {
                input: Box::new(scan(4, &[(1, 0), (2, 1)])),
                keys: vec![key(0, true)],
            }),
            limit: 10,
        },
        &[0, 1],
    );
    let actual: Vec<Vec<Value>> = capped
        .iter()
        .flat_map(|batch| {
            (0..batch.len()).map(move |row| vec![batch.value(row, s(0)), batch.value(row, s(1))])
        })
        .collect();
    assert_eq!(actual, expected);
}

/// `TopK` with k = 0 does not read its input at all.
#[test]
fn top_k_zero_reads_nothing() {
    let source = noisy(100);
    let pages = source.pages_served.clone();
    let plan = PhysicalPlan::TopK {
        input: Box::new(scan(4, &[(1, 0)])),
        keys: vec![key(0, false)],
        limit: 0,
    };
    assert!(run_with(source, plan, ExecutionContext::new(CommitTs(1)))
        .unwrap()
        .is_empty());
    assert_eq!(pages.load(Ordering::Relaxed), 0);
}

/// The whole relational pipeline: join, group, aggregate, top-k.
#[test]
fn join_aggregate_top_k_pipeline() {
    // regions (entity 5): region -> manager; total amount per manager, best first.
    let mut rows = sales().rows;
    rows.extend(
        [("north", "nina"), ("south", "sam")].map(|(region, manager)| {
            (
                RowId::compose(5, region.len() as u64 + manager.len() as u64),
                row(&[(1, t(region)), (2, t(manager))]),
            )
        }),
    );
    let plan = PhysicalPlan::TopK {
        input: Box::new(PhysicalPlan::Aggregate {
            input: Box::new(PhysicalPlan::HashJoin {
                left: Box::new(sales_scan()),
                right: Box::new(scan(5, &[(1, 20), (2, 21)])),
                join_type: JoinType::Inner,
                keys: vec![JoinKey {
                    left: s(1),
                    right: s(20),
                }],
                residual: None,
            }),
            group_by: vec![s(21)],
            aggregates: vec![agg(AggregateFunction::Sum, Some(2), 30)],
        }),
        keys: vec![key(30, true)],
        limit: 1,
    };
    assert_eq!(
        rows_of(MemorySource::new(rows), plan, &[21, 30]),
        vec![vec![t("nina"), i(60)]]
    );
}
