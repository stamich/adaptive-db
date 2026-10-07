//! Turns a physical plan into a pull-based operator tree and exposes it as a cursor.
use std::sync::Arc;

use adb_core::KeyRange;
use parking_lot::Mutex;

use crate::{
    operator::{
        filter::FilterOperator, limit::LimitOperator, point_lookup::PointLookupOperator,
        project::ProjectOperator, scan::ScanOperator, Operator,
    },
    DataSource, ExecutionContext, ExecutionError, PhysicalPlan, QueryMetrics, RecordBatch, SlotId,
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
        let shape = plan.validate().map_err(ExecutionError::InvalidPlan)?;
        let root = build_operator(source, plan, shape.width)?;

        Ok(QueryCursor {
            root,
            output: shape.output,
            context,
            metrics: Mutex::new(QueryMetrics::default()),
        })
    }
}

/// A running query: pull batches until `None`, cancel from any thread.
pub struct QueryCursor {
    /// Root of the operator tree.
    root: Box<dyn Operator>,
    /// Output slots of the root, in output-column order.
    output: Vec<SlotId>,
    /// Snapshot, limits and cancellation of the query.
    context: ExecutionContext,
    /// Counters updated on every batch.
    metrics: Mutex<QueryMetrics>,
}

impl QueryCursor {
    /// Next columnar batch, or `None` at the end; enforces the per-batch memory limit.
    pub fn next_batch(&mut self) -> Result<Option<RecordBatch>, ExecutionError> {
        self.context.check_running()?;
        let Some(rows) = self.root.next_batch(&self.context)? else {
            return Ok(None);
        };

        let batch = RecordBatch::from_rows(&rows, &self.output)?;
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

    /// Output slots of the query, in output-column order.
    pub fn output_slots(&self) -> &[SlotId] {
        &self.output
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

/// Recursively instantiates the operator for each plan node; rows are `width` slots wide.
fn build_operator(
    source: Arc<dyn DataSource>,
    plan: PhysicalPlan,
    width: usize,
) -> Result<Box<dyn Operator>, ExecutionError> {
    let child = |input: Box<PhysicalPlan>| build_operator(source.clone(), *input, width);
    Ok(match plan {
        PhysicalPlan::PointLookup { row_id, columns } => Box::new(PointLookupOperator::new(
            source.clone(),
            row_id,
            columns,
            width,
        )),

        PhysicalPlan::Scan { columns } => Box::new(ScanOperator::new(
            source.clone(),
            KeyRange::all(),
            columns,
            width,
        )),

        PhysicalPlan::EntityScan { entity_id, columns } => Box::new(ScanOperator::new(
            source.clone(),
            KeyRange::entity(entity_id),
            columns,
            width,
        )),

        PhysicalPlan::Filter { input, predicate } => {
            Box::new(FilterOperator::new(child(input)?, predicate))
        }

        PhysicalPlan::Project { input, slots } => {
            Box::new(ProjectOperator::new(child(input)?, slots))
        }

        PhysicalPlan::Limit { input, limit } => Box::new(LimitOperator::new(child(input)?, limit)),
    })
}
