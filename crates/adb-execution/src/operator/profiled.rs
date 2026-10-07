//! Wrapper that measures every operator of a query.
use std::time::{Duration, Instant};

use crate::{
    operator::{Operator, RowBatch},
    profile::{micros, OperatorProfile},
    ExecutionContext, ExecutionError,
};

/// Counts the rows and batches an operator returns and the time its pulls take.
///
/// The executor wraps every operator, so operators themselves only report what is specific to
/// them (see [`Operator::counters`]).
pub struct Profiled {
    /// The measured operator.
    inner: Box<dyn Operator>,
    /// Rows returned so far.
    rows: u64,
    /// Batches returned so far.
    batches: u64,
    /// Time spent in `next_batch` (inclusive of the inputs).
    elapsed: Duration,
}

impl Profiled {
    /// Wraps `inner` so that it is measured.
    pub fn wrap(inner: Box<dyn Operator>) -> Box<dyn Operator> {
        Box::new(Self {
            inner,
            rows: 0,
            batches: 0,
            elapsed: Duration::ZERO,
        })
    }
}

impl Operator for Profiled {
    /// Delegates and records the outcome.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        let started = Instant::now();
        let result = self.inner.next_batch(context);
        self.elapsed += started.elapsed();
        if let Ok(Some(batch)) = &result {
            self.rows += batch.len() as u64;
            self.batches += 1;
        }
        result
    }

    /// Name of the measured operator.
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    /// Inputs of the measured operator.
    fn children(&self) -> Vec<&dyn Operator> {
        self.inner.children()
    }

    /// Counters of the measured operator.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        self.inner.counters()
    }

    /// The measured operator's profile with rows, batches and time filled in.
    fn profile(&self) -> OperatorProfile {
        let mut profile = self.inner.profile();
        profile.rows_out = self.rows;
        profile.batches_out = self.batches;
        profile.elapsed_us = micros(self.elapsed);
        profile
    }
}
