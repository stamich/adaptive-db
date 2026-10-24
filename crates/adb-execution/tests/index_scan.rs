//! IndexScan and RowFetch operators over an emulated index.
mod support;

use adb_core::{CommitTs, Direction, IndexId, RowId, Value};
use adb_execution::{ExecutionContext, Executor, PhysicalPlan};
use adb_index::{
    IndexBounds, IndexColumn, IndexDefinition, IndexSpec, IndexState, ScanDirection, ValueBound,
};
use std::sync::{atomic::Ordering, Arc};
use support::{cols, i, row, rows_of, run_with, t, MemorySource};

/// Entity of the accounts.
const ACCOUNT: u64 = 2;

/// account(id, balance = id % 7, owner, note) with ids -20..30; index on balance INCLUDE owner.
fn source() -> MemorySource {
    let rows = (-20..30i64)
        .map(|id| {
            (
                RowId::from_primary_key(ACCOUNT, id),
                row(&[
                    (1, i(id)),
                    (2, i(id.rem_euclid(7))),
                    (3, t(&format!("o{id}"))),
                    (4, i(id * 10)),
                ]),
            )
        })
        .collect();
    MemorySource::new(rows).with_index(IndexDefinition::new(
        IndexId(4),
        IndexSpec {
            name: "idx_balance".into(),
            entity_id: ACCOUNT,
            columns: vec![IndexColumn {
                field_id: 2,
                direction: Direction::Asc,
            }],
            include: vec![3],
        },
        IndexState::Active,
        CommitTs(0),
    ))
}

/// `IndexScan` over idx_balance with `bounds` reading balance and owner into slots 0 and 1.
fn index_scan(bounds: IndexBounds, direction: ScanDirection, limit: Option<usize>) -> PhysicalPlan {
    PhysicalPlan::IndexScan {
        index_id: 4,
        bounds,
        direction,
        columns: cols(&[(2, 0), (3, 1)]),
        limit,
    }
}

/// A covering equality scan returns the matching rows in primary-key order.
#[test]
fn covering_equality_scan() {
    let bounds = IndexBounds {
        prefix: vec![i(3)],
        ..IndexBounds::all()
    };
    let rows = rows_of(
        source(),
        index_scan(bounds, ScanDirection::Forward, None),
        &[0, 1],
    );
    let owners: Vec<Value> = rows.iter().map(|r| r[1].clone()).collect();
    assert_eq!(
        owners,
        vec![
            t("o-18"),
            t("o-11"),
            t("o-4"),
            t("o3"),
            t("o10"),
            t("o17"),
            t("o24")
        ]
    );
    assert!(rows.iter().all(|r| r[0] == i(3)));
}

/// Range scans honour inclusive and exclusive bounds and return index order; backward scans
/// return the reverse; a limit hint keeps the first page small.
#[test]
fn range_scans_in_both_directions() {
    let bounds = IndexBounds {
        prefix: vec![],
        lower: Some(ValueBound {
            value: i(5),
            inclusive: false,
        }),
        upper: None,
    };
    let forward = rows_of(
        source(),
        index_scan(bounds.clone(), ScanDirection::Forward, None),
        &[0],
    );
    assert_eq!(forward.len(), 7, "balance 6 only");
    let bounds = IndexBounds {
        prefix: vec![],
        lower: Some(ValueBound {
            value: i(4),
            inclusive: true,
        }),
        upper: Some(ValueBound {
            value: i(5),
            inclusive: true,
        }),
    };
    let forward = rows_of(
        source(),
        index_scan(bounds.clone(), ScanDirection::Forward, None),
        &[0, 1],
    );
    let backward = rows_of(
        source(),
        index_scan(bounds, ScanDirection::Backward, None),
        &[0, 1],
    );
    assert_eq!(forward.len(), 14);
    let balance = |value: &Value| match value {
        Value::Int64(v) => *v,
        other => panic!("{other:?}"),
    };
    assert!(forward
        .windows(2)
        .all(|pair| balance(&pair[0][0]) <= balance(&pair[1][0])));
    let mut reversed = backward.clone();
    reversed.reverse();
    assert_eq!(reversed, forward);

    let limited = PhysicalPlan::Limit {
        input: Box::new(index_scan(
            IndexBounds::all(),
            ScanDirection::Forward,
            Some(3),
        )),
        limit: 3,
    };
    let source = source();
    let pages = source.pages_served.clone();
    let mut context = ExecutionContext::new(CommitTs(1));
    context.batch_size = 1024;
    let batches = run_with(source, limited, context).unwrap();
    assert_eq!(batches.iter().map(|b| b.len()).sum::<usize>(), 3);
    assert_eq!(
        pages.load(Ordering::Relaxed),
        0,
        "index scans do not use table scans"
    );
}

/// A non-covering scan fetches the rows in batches and keeps index order; the profile reports
/// fetch batches and examined entries.
#[test]
fn row_fetch_reads_the_remaining_columns() {
    let source = source();
    let fetches = source.fetches_served.clone();
    let plan = PhysicalPlan::RowFetch {
        input: Box::new(index_scan(
            IndexBounds {
                prefix: vec![i(0)],
                ..IndexBounds::all()
            },
            ScanDirection::Forward,
            None,
        )),
        columns: cols(&[(1, 2), (4, 3)]),
    };
    let mut cursor =
        Executor::execute(Arc::new(source), plan, ExecutionContext::new(CommitTs(1))).unwrap();
    let mut rows = Vec::new();
    while let Some(batch) = cursor.next_batch().unwrap() {
        for r in 0..batch.len() {
            rows.push((
                batch.value(r, adb_execution::SlotId(2)),
                batch.value(r, adb_execution::SlotId(3)),
            ));
        }
    }
    let ids: Vec<i64> = rows
        .iter()
        .map(|(id, _)| match id {
            Value::Int64(v) => *v,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![-14, -7, 0, 7, 14, 21, 28]);
    assert!(rows.iter().all(|(id, note)| match (id, note) {
        (Value::Int64(id), Value::Int64(note)) => *note == id * 10,
        _ => false,
    }));
    assert_eq!(fetches.load(Ordering::Relaxed), 1, "one batch, one fetch");
    let profile = cursor.profile();
    assert_eq!(profile.root.operator, "row_fetch");
    assert_eq!(profile.root.counters.get("fetch_batches"), Some(&1));
    let scan = &profile.root.children[0];
    assert_eq!(scan.operator, "index_scan");
    assert_eq!(scan.counters.get("entries_examined"), Some(&7));
}

/// Plans are validated: fetched slots must be new, limits bounded; sources without indexes
/// report scans as unsupported.
#[test]
fn plans_are_validated() {
    let duplicate = PhysicalPlan::RowFetch {
        input: Box::new(index_scan(IndexBounds::all(), ScanDirection::Forward, None)),
        columns: cols(&[(1, 0)]),
    };
    assert!(duplicate.validate().is_err());
    let too_large = index_scan(IndexBounds::all(), ScanDirection::Forward, Some(2_000_000));
    assert!(too_large.validate().is_err());
    let plain = MemorySource::new(vec![]);
    assert!(run_with(
        plain,
        index_scan(IndexBounds::all(), ScanDirection::Forward, None),
        ExecutionContext::new(CommitTs(1))
    )
    .is_err());
    assert_eq!(
        index_scan(IndexBounds::all(), ScanDirection::Forward, None).node_count(),
        1
    );
    assert_eq!(duplicate.output_slots().len(), 3);
}
