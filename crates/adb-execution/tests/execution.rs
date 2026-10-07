//! Operator pipelines over an in-memory data source.
mod support;

use std::sync::{atomic::Ordering, Arc};

use adb_core::{CommitTs, RowId, Value};
use adb_execution::{
    BinaryOp, ExecutionContext, ExecutionError, Executor, Expr, PhysicalPlan, SlotId,
};
use support::*;

/// Rows 1..=100 of entity 0: field 1 = primary key, field 2 = a label.
fn source() -> MemorySource {
    numbered((1..=100).map(RowId))
}

/// One row per id: field 1 = primary key, field 2 = `row-<id>`.
fn numbered(ids: impl IntoIterator<Item = RowId>) -> MemorySource {
    MemorySource::new(
        ids.into_iter()
            .map(|id| {
                (
                    id,
                    row(&[
                        (1, i(id.primary_key() as i64)),
                        (2, t(&format!("row-{}", id.0))),
                    ]),
                )
            })
            .collect(),
    )
}

/// Full scan of entity 0 with field 1 -> slot 0 and field 2 -> slot 1.
fn scan_all() -> PhysicalPlan {
    PhysicalPlan::Scan {
        columns: cols(&[(1, 0), (2, 1)]),
    }
}

/// Scan, filter, project and limit compose into one bounded batch.
#[test]
fn scan_filter_project_limit_pipeline() {
    let plan = PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::Project {
            input: Box::new(PhysicalPlan::Filter {
                input: Box::new(scan_all()),
                predicate: Expr::Binary {
                    left: Box::new(Expr::Slot { slot: s(0) }),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Literal {
                        value: Value::Int64(50),
                    }),
                },
            }),
            slots: vec![s(0)],
        }),
        limit: 10,
    };

    let mut cursor =
        Executor::execute(Arc::new(source()), plan, ExecutionContext::new(CommitTs(1))).unwrap();

    let batch = cursor.next_batch().unwrap().unwrap();
    assert_eq!(batch.len(), 10);
    assert_eq!(batch.columns.len(), 1);
    assert_eq!(batch.columns[0].slot(), s(0));
    assert_eq!(batch.value(0, s(0)), i(51));
    assert!(
        batch.row_ids.is_some(),
        "scan output keeps its storage keys"
    );
    assert!(cursor.next_batch().unwrap().is_none());
}

/// A scan writes each field into the slot the plan assigns, so two relation instances of the
/// same entity never collide even though they share field ids.
#[test]
fn scan_maps_fields_to_assigned_slots() {
    let plan = PhysicalPlan::Project {
        input: Box::new(PhysicalPlan::Limit {
            input: Box::new(PhysicalPlan::Scan {
                columns: cols(&[(2, 7), (1, 3)]),
            }),
            limit: 1,
        }),
        slots: vec![s(7), s(3)],
    };
    let rows = rows_of(source(), plan, &[7, 3]);
    assert_eq!(rows, vec![vec![t("row-1"), i(1)]]);
}

/// The batch columns follow the projection order, not the slot numbering.
#[test]
fn output_columns_follow_projection_order() {
    let plan = PhysicalPlan::Project {
        input: Box::new(scan_all()),
        slots: vec![s(1), s(0)],
    };
    let mut cursor =
        Executor::execute(Arc::new(source()), plan, ExecutionContext::new(CommitTs(1))).unwrap();
    assert_eq!(cursor.output_slots(), &[s(1), s(0)]);
    let batch = cursor.next_batch().unwrap().unwrap();
    let order: Vec<SlotId> = batch.columns.iter().map(|column| column.slot()).collect();
    assert_eq!(order, vec![s(1), s(0)]);
}

/// A cancelled cursor fails its next pull.
#[test]
fn cancellation_is_observed() {
    let mut cursor = Executor::execute(
        Arc::new(source()),
        scan_all(),
        ExecutionContext::new(CommitTs(1)),
    )
    .unwrap();

    cursor.cancel();
    assert!(matches!(
        cursor.next_batch(),
        Err(ExecutionError::Cancelled)
    ));
}

/// `EntityScan` reads exactly one entity's key range.
#[test]
fn entity_scan_returns_only_that_entity() {
    let rows = (1..=3u64).flat_map(|entity| (0..10u64).map(move |pk| RowId::compose(entity, pk)));
    let mut cursor = Executor::execute(
        Arc::new(numbered(rows)),
        scan(2, &[(1, 0)]),
        ExecutionContext::new(CommitTs(1)),
    )
    .unwrap();
    let batch = cursor.next_batch().unwrap().unwrap();
    assert_eq!(batch.len(), 10);
    assert!(batch
        .row_ids
        .unwrap()
        .iter()
        .all(|row_id| row_id.entity_id() == 2));
    assert!(cursor.next_batch().unwrap().is_none());
}

/// Point lookups map their fields into slots like scans do.
#[test]
fn point_lookup_maps_fields_to_slots() {
    let plan = PhysicalPlan::PointLookup {
        row_id: RowId(42),
        columns: cols(&[(2, 0)]),
    };
    assert_eq!(rows_of(source(), plan, &[0]), vec![vec![t("row-42")]]);
}

/// Scans pull pages lazily: a LIMIT that is satisfied by the first page never reads the rest,
/// and a full scan is served in batch-sized pages instead of one materialized vector.
#[test]
fn scans_are_paged_and_lazy() {
    let source = numbered((1..=1000).map(RowId));
    let pages = Arc::clone(&source.pages_served);
    let mut context = ExecutionContext::new(CommitTs(1));
    context.batch_size = 100;

    let mut limited = Executor::execute(
        Arc::new(source.clone()),
        PhysicalPlan::Limit {
            input: Box::new(scan_all()),
            limit: 5,
        },
        context.clone(),
    )
    .unwrap();
    while limited.next_batch().unwrap().is_some() {}
    assert_eq!(pages.load(Ordering::Relaxed), 1);

    pages.store(0, Ordering::Relaxed);
    let mut full = Executor::execute(Arc::new(source), scan_all(), context).unwrap();
    let mut rows = 0;
    while let Some(batch) = full.next_batch().unwrap() {
        assert!(batch.len() <= 100);
        rows += batch.len();
    }
    assert_eq!(rows, 1000);
    assert_eq!(
        pages.load(Ordering::Relaxed),
        11,
        "10 full pages + 1 empty terminator"
    );
}
