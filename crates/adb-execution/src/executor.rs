//! Module `executor` for crate `adb-execution`.
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    operator::{
        filter::FilterOperator, limit::LimitOperator, point_lookup::PointLookupOperator,
        project::ProjectOperator, scan::ScanOperator, Operator,
    },
    DataSource, ExecutionContext, ExecutionError, PhysicalPlan, QueryMetrics, RecordBatch,
};

/// Represents `Executor` state used by this subsystem.
pub struct Executor;

/// Implements behavior for `Executor`.
impl Executor {
    /// Implements the `execute` operation used by this subsystem.
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

/// Represents `QueryCursor` state used by this subsystem.
pub struct QueryCursor {
    root: Box<dyn Operator>,
    context: ExecutionContext,
    metrics: Mutex<QueryMetrics>,
}

/// Implements behavior for `QueryCursor`.
impl QueryCursor {
    /// Implements the `next_batch` operation used by this subsystem.
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

    /// Implements the `cancel` operation used by this subsystem.
    pub fn cancel(&self) {
        self.context.cancellation.cancel();
    }

    /// Implements the `metrics` operation used by this subsystem.
    pub fn metrics(&self) -> QueryMetrics {
        *self.metrics.lock()
    }

    /// Implements the `cancellation_token` operation used by this subsystem.
    pub fn cancellation_token(&self) -> crate::CancellationToken {
        self.context.cancellation.clone()
    }
}

/// Implements the `build_operator` operation used by this subsystem.
fn build_operator(
    source: Arc<dyn DataSource>,
    plan: PhysicalPlan,
) -> Result<Box<dyn Operator>, ExecutionError> {
    Ok(match plan {
        PhysicalPlan::PointLookup { row_id } => Box::new(PointLookupOperator::new(source, row_id)),

        PhysicalPlan::Scan => Box::new(ScanOperator::new(source)),

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
