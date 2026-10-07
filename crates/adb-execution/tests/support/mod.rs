//! Shared fixtures of the execution tests: an in-memory data source and plan helpers.
#![allow(dead_code)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use adb_core::{CommitTs, KeyRange, Row, RowId, Value};
use adb_execution::{
    DataSource, ExecutionContext, ExecutionError, Executor, PhysicalPlan, RecordBatch, ScanColumn,
    SlotId,
};

/// In-memory data source that counts the scan pages it serves.
#[derive(Clone)]
pub struct MemorySource {
    /// Rows in key order.
    pub rows: Vec<(RowId, Row)>,
    /// Number of `scan_page` calls served.
    pub pages_served: Arc<AtomicUsize>,
}

impl MemorySource {
    /// A source holding `rows`, sorted by key.
    pub fn new(mut rows: Vec<(RowId, Row)>) -> Self {
        rows.sort_by_key(|(id, _)| *id);
        Self {
            rows,
            pages_served: Arc::new(AtomicUsize::new(0)),
        }
    }
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

/// Shorthand for a slot id.
pub fn s(id: u32) -> SlotId {
    SlotId(id)
}

/// Scan columns mapping `(field_id, slot)` pairs.
pub fn cols(pairs: &[(u32, u32)]) -> Vec<ScanColumn> {
    pairs
        .iter()
        .map(|(field_id, slot)| ScanColumn {
            field_id: *field_id,
            slot: SlotId(*slot),
        })
        .collect()
}

/// Entity scan of `entity` with the given field-to-slot mapping.
pub fn scan(entity: u64, pairs: &[(u32, u32)]) -> PhysicalPlan {
    PhysicalPlan::EntityScan {
        entity_id: entity,
        columns: cols(pairs),
    }
}

/// A stored row built from `(field, value)` pairs.
pub fn row(fields: &[(u32, Value)]) -> Row {
    fields.iter().fold(Row::new(), |row, (field, value)| {
        row.with_field(*field, value.clone())
    })
}

/// Runs `plan` with `context` and returns every output batch.
pub fn run_with(
    source: MemorySource,
    plan: PhysicalPlan,
    context: ExecutionContext,
) -> Result<Vec<RecordBatch>, ExecutionError> {
    let mut cursor = Executor::execute(Arc::new(source), plan, context)?;
    let mut out = Vec::new();
    while let Some(batch) = cursor.next_batch()? {
        out.push(batch);
    }
    Ok(out)
}

/// Runs `plan` with default settings and returns the output as rows of values, one entry per
/// requested slot (NULL when the column is absent).
pub fn rows_of(source: MemorySource, plan: PhysicalPlan, slots: &[u32]) -> Vec<Vec<Value>> {
    let batches = run_with(source, plan, ExecutionContext::new(CommitTs(1))).unwrap();
    batches
        .iter()
        .flat_map(|batch| {
            (0..batch.len()).map(move |row| {
                slots
                    .iter()
                    .map(|slot| batch.value(row, SlotId(*slot)))
                    .collect()
            })
        })
        .collect()
}

/// Shorthand for an `Int64` value.
pub fn i(value: i64) -> Value {
    Value::Int64(value)
}

/// Shorthand for a `String` value.
pub fn t(value: &str) -> Value {
    Value::String(value.to_string())
}
