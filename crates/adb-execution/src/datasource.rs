//! What the executor needs from storage.
use adb_core::{CommitTs, FieldId, IndexId, KeyRange, Row, RowId, SchemaEpoch};
use adb_index::{IndexBounds, ScanDirection};

use crate::ExecutionError;

/// What a read observes: the data snapshot, and (since 2.4.0) the catalog epoch whose schemas
/// rows are read through.
///
/// A query pins both when it starts. Rows written under any schema version are adapted to the
/// version of their entity that was current at `schema_epoch`, so a concurrent `ALTER TABLE`
/// never changes what a running query sees. `None` reads through the latest schemas (internal
/// reads such as `ANALYZE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadView {
    /// Data snapshot (MVCC commit timestamp).
    pub snapshot_ts: CommitTs,
    /// Catalog epoch the schemas are pinned to; `None` for the latest.
    pub schema_epoch: Option<SchemaEpoch>,
}

impl ReadView {
    /// A view at `snapshot_ts` pinned to the schemas of `schema_epoch`.
    pub fn pinned(snapshot_ts: CommitTs, schema_epoch: SchemaEpoch) -> Self {
        Self {
            snapshot_ts,
            schema_epoch: Some(schema_epoch),
        }
    }
}

impl From<CommitTs> for ReadView {
    /// A view at `snapshot_ts` through the latest schemas.
    fn from(snapshot_ts: CommitTs) -> Self {
        Self {
            snapshot_ts,
            schema_epoch: None,
        }
    }
}

/// Snapshot-consistent row access. The executor knows nothing about pages, heaps, logs or
/// transactions; it only asks for rows visible in a [`ReadView`].
pub trait DataSource: IndexAccess + Send + Sync {
    /// Latest visible commit (default snapshot of a query).
    fn latest_committed_ts(&self) -> CommitTs;

    /// The row `row_id` as seen by `view`.
    fn point_lookup(&self, row_id: RowId, view: ReadView) -> Result<Option<Row>, ExecutionError>;

    /// Up to `limit` rows of `range` visible in `view`, in key order, with keys strictly
    /// after `after`. An empty result means the range is exhausted.
    ///
    /// Scans are paged with this keyset cursor so memory stays bounded by the batch size.
    fn scan_page(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
        view: ReadView,
    ) -> Result<Vec<(RowId, Row)>, ExecutionError>;
}

/// One page of an index scan: the matching entries visible at the snapshot, as partial rows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IndexPage {
    /// For every entry in scan order: its row and the requested key and covered fields (a
    /// partial row; fields that are NULL are absent).
    pub rows: Vec<(RowId, Row)>,
    /// Opaque position to resume after, or `None` when the scan is complete.
    pub resume: Option<Vec<u8>>,
    /// Index entries examined (visible or not).
    pub examined: u64,
}

/// Index access (Milestone 2.3.0): bounded, resumable index scans and batched row fetches.
///
/// Sources without indexes (test doubles) keep the default methods, which report the
/// operation as unsupported.
pub trait IndexAccess {
    /// One page of a scan of index `index`: up to `limit` entries matching `bounds` that are
    /// visible in `view`, in `direction`, after the position `resume` of the previous
    /// page. `fields` are the key and covered fields the caller reads; a field the index does
    /// not hold is an error (plans read other fields through a row fetch).
    ///
    /// Fails if the index does not exist or cannot serve the snapshot (built later, or its
    /// history was vacuumed); a scan checks this on every page.
    #[allow(clippy::too_many_arguments)]
    fn index_scan_page(
        &self,
        index: IndexId,
        bounds: &IndexBounds,
        direction: ScanDirection,
        fields: &[FieldId],
        resume: Option<&[u8]>,
        limit: usize,
        view: ReadView,
    ) -> Result<IndexPage, ExecutionError> {
        let _ = (index, bounds, direction, fields, resume, limit, view);
        Err(ExecutionError::DataSource(
            "this data source has no indexes".into(),
        ))
    }

    /// The rows `row_ids` (in ascending order) as seen by `view`, in the same order; `None`
    /// for a row that is not visible. Implementations visit each storage page once.
    fn fetch_rows(
        &self,
        row_ids: &[RowId],
        view: ReadView,
    ) -> Result<Vec<Option<Row>>, ExecutionError> {
        let _ = (row_ids, view);
        Err(ExecutionError::DataSource(
            "this data source cannot fetch rows in batches".into(),
        ))
    }
}
