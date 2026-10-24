//! Shared fixtures of the execution tests: an in-memory data source and plan helpers.
#![allow(dead_code)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use adb_core::{CommitTs, FieldId, IndexId, KeyRange, Row, RowId, Value};
use adb_execution::{
    DataSource, ExecutionContext, ExecutionError, Executor, IndexAccess, IndexPage, PhysicalPlan,
    RecordBatch, ScanColumn, SlotId,
};
use adb_index::{
    entry::{encode_included, encode_index_key, tree_key},
    IndexBounds, IndexDefinition, ScanDirection,
};

/// In-memory data source that counts the scan pages it serves; it can also emulate secondary
/// indexes (every row visible, entries built on demand with the real key encoding).
#[derive(Clone)]
pub struct MemorySource {
    /// Rows in key order.
    pub rows: Vec<(RowId, Row)>,
    /// Number of `scan_page` calls served.
    pub pages_served: Arc<AtomicUsize>,
    /// Emulated indexes.
    pub indexes: Vec<IndexDefinition>,
    /// Number of `fetch_rows` calls served.
    pub fetches_served: Arc<AtomicUsize>,
}

impl MemorySource {
    /// A source holding `rows`, sorted by key.
    pub fn new(mut rows: Vec<(RowId, Row)>) -> Self {
        rows.sort_by_key(|(id, _)| *id);
        Self {
            rows,
            pages_served: Arc::new(AtomicUsize::new(0)),
            indexes: Vec::new(),
            fetches_served: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The same source with an emulated index.
    pub fn with_index(mut self, definition: IndexDefinition) -> Self {
        self.indexes.push(definition);
        self
    }
}

impl IndexAccess for MemorySource {
    /// Builds the sorted entries of the index, then pages through the matching ones.
    fn index_scan_page(
        &self,
        index: IndexId,
        bounds: &IndexBounds,
        direction: ScanDirection,
        fields: &[FieldId],
        resume: Option<&[u8]>,
        limit: usize,
        _snapshot_ts: CommitTs,
    ) -> Result<IndexPage, ExecutionError> {
        let fail = |message: String| ExecutionError::DataSource(message);
        let definition = self
            .indexes
            .iter()
            .find(|definition| definition.id == index)
            .ok_or_else(|| fail(format!("no index {}", index.0)))?;
        let range = bounds
            .byte_range(definition)
            .map_err(|e| fail(e.to_string()))?;
        let mut entries: Vec<(Vec<u8>, RowId, Row)> = self
            .rows
            .iter()
            .filter(|(id, _)| id.entity_id() == definition.entity_id)
            .map(|(id, row)| {
                let key = encode_index_key(definition, &definition.key_values(row)).unwrap();
                encode_included(definition, &definition.included_values(row)).unwrap();
                (tree_key(&key, *id, CommitTs(0)), *id, row.clone())
            })
            .filter(|(key, _, _)| range.contains(key))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if direction == ScanDirection::Backward {
            entries.reverse();
        }
        let start = match resume {
            Some(after) => entries
                .iter()
                .position(|(key, _, _)| key.as_slice() == after)
                .map_or(0, |at| at + 1),
            None => 0,
        };
        let page: Vec<&(Vec<u8>, RowId, Row)> = entries[start..].iter().take(limit).collect();
        let resume = (start + page.len() < entries.len()).then(|| page.last().unwrap().0.clone());
        Ok(IndexPage {
            examined: page.len() as u64,
            rows: page
                .into_iter()
                .map(|(_, id, row)| {
                    let mut partial = Row::new();
                    for field in fields {
                        if let Some(value) = row.get(*field) {
                            partial.fields.insert(*field, value.clone());
                        }
                    }
                    (*id, partial)
                })
                .collect(),
            resume,
        })
    }

    /// Looks every row up; counts the calls.
    fn fetch_rows(
        &self,
        row_ids: &[RowId],
        _snapshot_ts: CommitTs,
    ) -> Result<Vec<Option<Row>>, ExecutionError> {
        self.fetches_served.fetch_add(1, Ordering::Relaxed);
        Ok(row_ids
            .iter()
            .map(|id| {
                self.rows
                    .iter()
                    .find(|(row_id, _)| row_id == id)
                    .map(|(_, row)| row.clone())
            })
            .collect())
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
