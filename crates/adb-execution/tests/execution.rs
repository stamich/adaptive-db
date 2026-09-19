//! Module `execution` for crate `adb-execution`.
use std::sync::Arc;

use adb_core::{CommitTs, Row, RowId, Value};
use adb_execution::{
    BinaryOp, DataSource, ExecutionContext, ExecutionError, Executor, Expr, PhysicalPlan,
};

/// Represents `MemorySource` state used by this subsystem.
#[derive(Clone)]
struct MemorySource {
    rows: Vec<(RowId, Row)>,
}

/// Implements behavior for `DataSource`.
impl DataSource for MemorySource {
    /// Implements the `latest_committed_ts` operation used by this subsystem.
    fn latest_committed_ts(&self) -> CommitTs {
        CommitTs(1)
    }

    /// Implements the `point_lookup` operation used by this subsystem.
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

    /// Implements the `scan_rows` operation used by this subsystem.
    fn scan_rows(&self, _snapshot_ts: CommitTs) -> Result<Vec<(RowId, Row)>, ExecutionError> {
        Ok(self.rows.clone())
    }
}

/// Implements the `source` operation used by this subsystem.
fn source() -> Arc<dyn DataSource> {
    Arc::new(MemorySource {
        rows: (1..=100)
            .map(|id| {
                (
                    RowId(id),
                    Row::new()
                        .with_field(1, Value::Int64(id as i64))
                        .with_field(2, Value::String(format!("row-{id}"))),
                )
            })
            .collect(),
    })
}

/// Implements the `scan_filter_project_limit_pipeline` operation used by this subsystem.
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

/// Implements the `cancellation_is_observed` operation used by this subsystem.
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
