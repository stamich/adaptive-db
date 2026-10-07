//! Operator pipelines over an in-memory data source.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use adb_core::{CommitTs, KeyRange, Row, RowId, Value};
use adb_execution::{
    BinaryOp, DataSource, ExecutionContext, ExecutionError, Executor, Expr, PhysicalPlan,
};

/// In-memory data source that counts the scan pages it serves.
#[derive(Clone)]
struct MemorySource {
    rows: Vec<(RowId, Row)>,
    pages_served: Arc<AtomicUsize>,
}

impl DataSource for MemorySource {
    /// Fixed snapshot of the in-memory source.
    fn latest_committed_ts(&self) -> CommitTs {
        CommitTs(1)
    }

    /// Linear search for `row_id`.
    fn point_lookup(
        &self,
        row_id: RowId,
        _snapshot_ts: CommitTs,
    ) -> Result<Option<Row>, ExecutionError> {
        Ok(self
            .rows
            .iter()
            .find(|(id, _)| *id == row_id)
            .map(|(_, row)| row.clone()))
    }

    /// Rows of `range` after `after`, at most `limit`; counts served pages.
    fn scan_page(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
        _snapshot_ts: CommitTs,
    ) -> Result<Vec<(RowId, Row)>, ExecutionError> {
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .rows
            .iter()
            .filter(|(id, _)| range.contains(*id) && after.is_none_or(|after| *id > after))
            .take(limit)
            .cloned()
            .collect())
    }
}

/// Rows 1..=100 of entity 0.
fn memory(rows: impl IntoIterator<Item = RowId>) -> MemorySource {
    MemorySource {
        rows: rows
            .into_iter()
            .map(|id| {
                (
                    id,
                    Row::new()
                        .with_field(1, Value::Int64(id.primary_key() as i64))
                        .with_field(2, Value::String(format!("row-{}", id.0))),
                )
            })
            .collect(),
        pages_served: Arc::new(AtomicUsize::new(0)),
    }
}

/// Rows 1..=100 of entity 0.
fn source() -> Arc<dyn DataSource> {
    Arc::new(memory((1..=100).map(RowId)))
}

/// Scan, filter, project and limit compose into one bounded batch.
#[test]
fn scan_filter_project_limit_pipeline() {
    let plan = PhysicalPlan::Limit {
        input: Box::new(PhysicalPlan::Project {
            input: Box::new(PhysicalPlan::Filter {
                input: Box::new(PhysicalPlan::Scan),
                predicate: Expr::Binary {
                    left: Box::new(Expr::Column { field_id: 1 }),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Literal {
                        value: Value::Int64(50),
                    }),
                },
            }),
            fields: vec![1],
        }),
        limit: 10,
    };

    let mut cursor = Executor::execute(source(), plan, ExecutionContext::new(CommitTs(1))).unwrap();

    let batch = cursor.next_batch().unwrap().unwrap();
    assert_eq!(batch.len(), 10);
    assert_eq!(batch.columns.len(), 1);
    assert!(cursor.next_batch().unwrap().is_none());
}

/// A cancelled cursor fails its next pull.
#[test]
fn cancellation_is_observed() {
    let mut cursor = Executor::execute(
        source(),
        PhysicalPlan::Scan,
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
        Arc::new(memory(rows)),
        PhysicalPlan::EntityScan { entity_id: 2 },
        ExecutionContext::new(CommitTs(1)),
    )
    .unwrap();
    let batch = cursor.next_batch().unwrap().unwrap();
    assert_eq!(batch.len(), 10);
    assert!(cursor.next_batch().unwrap().is_none());
}

/// Scans pull pages lazily: a LIMIT that is satisfied by the first page never reads the rest,
/// and a full scan is served in batch-sized pages instead of one materialized vector.
#[test]
fn scans_are_paged_and_lazy() {
    let source = memory((1..=1000).map(RowId));
    let pages = Arc::clone(&source.pages_served);
    let mut context = ExecutionContext::new(CommitTs(1));
    context.batch_size = 100;

    let mut limited = Executor::execute(
        Arc::new(source.clone()),
        PhysicalPlan::Limit {
            input: Box::new(PhysicalPlan::Scan),
            limit: 5,
        },
        context.clone(),
    )
    .unwrap();
    while limited.next_batch().unwrap().is_some() {}
    assert_eq!(pages.load(Ordering::Relaxed), 1);

    pages.store(0, Ordering::Relaxed);
    let mut full = Executor::execute(Arc::new(source), PhysicalPlan::Scan, context).unwrap();
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
