//! Turns a physical plan into a pull-based operator tree and exposes it as a cursor.
use std::sync::Arc;

use adb_core::KeyRange;
use parking_lot::Mutex;

use crate::{
    operator::{
        filter::FilterOperator, limit::LimitOperator, point_lookup::PointLookupOperator,
        project::ProjectOperator, scan::ScanOperator, Operator,
    },
    DataSource, ExecutionContext, ExecutionError, PhysicalPlan, QueryMetrics, RecordBatch,
};

/// Plan validation and operator-tree construction.
pub struct Executor;

impl Executor {
    /// Validates `plan` and returns a cursor that produces its batches lazily.
    pub fn execute(
        source: Arc<dyn DataSource>,
        plan: PhysicalPlan,
        context: ExecutionContext,
    ) -> Result<QueryCursor, ExecutionError> {
        plan.validate().map_err(ExecutionError::InvalidPlan)?;
        let root = build_operator(source, plan)?;

        Ok(QueryCursor {
            root,
            context,
            metrics: Mutex::new(QueryMetrics::default()),
        })
    }
}

/// A running query: pull batches until `None`, cancel from any thread.
pub struct QueryCursor {
    root: Box<dyn Operator>,
    context: ExecutionContext,
    metrics: Mutex<QueryMetrics>,
}

impl QueryCursor {
    /// Next columnar batch, or `None` at the end; enforces the per-batch memory limit.
    pub fn next_batch(&mut self) -> Result<Option<RecordBatch>, ExecutionError> {
        self.context.check_running()?;
        let Some(rows) = self.root.next_batch(&self.context)? else {
            return Ok(None);
        };

        let batch = RecordBatch::from_rows(&rows)?;
        let estimated = batch.estimated_heap_bytes();
        if estimated > self.context.memory_limit_bytes {
            return Err(ExecutionError::ResourceLimit(format!(
                "batch requires approximately {estimated} bytes, limit is {}",
                self.context.memory_limit_bytes
            )));
        }
        let mut metrics = self.metrics.lock();
        metrics.output_rows += batch.len() as u64;
        metrics.batches += 1;
        Ok(Some(batch))
    }

    /// Requests cooperative cancellation; the next pull fails with `Cancelled`.
    pub fn cancel(&self) {
        self.context.cancellation.cancel();
    }

    /// Counters accumulated so far.
    pub fn metrics(&self) -> QueryMetrics {
        *self.metrics.lock()
    }

    /// Token that cancels this query from another thread.
    pub fn cancellation_token(&self) -> crate::CancellationToken {
        self.context.cancellation.clone()
    }
}

/// Recursively instantiates the operator for each plan node.
fn build_operator(
    source: Arc<dyn DataSource>,
    plan: PhysicalPlan,
) -> Result<Box<dyn Operator>, ExecutionError> {
    Ok(match plan {
        PhysicalPlan::PointLookup { row_id } => Box::new(PointLookupOperator::new(source, row_id)),

        PhysicalPlan::Scan => Box::new(ScanOperator::new(source, KeyRange::all())),

        PhysicalPlan::EntityScan { entity_id } => {
            Box::new(ScanOperator::new(source, KeyRange::entity(entity_id)))
        }

        PhysicalPlan::Filter { input, predicate } => Box::new(FilterOperator::new(
            build_operator(source, *input)?,
            predicate,
        )),

        PhysicalPlan::Project { input, fields } => Box::new(ProjectOperator::new(
            build_operator(source, *input)?,
            fields,
        )),

        PhysicalPlan::Limit { input, limit } => {
            Box::new(LimitOperator::new(build_operator(source, *input)?, limit))
        }
    })
}
