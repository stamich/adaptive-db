//! What the executor needs from storage.
use adb_core::{CommitTs, FieldId, IndexId, KeyRange, Row, RowId};
use adb_index::{IndexBounds, ScanDirection};

use crate::ExecutionError;

/// Snapshot-consistent row access. The executor knows nothing about pages, heaps, logs or
/// transactions; it only asks for rows visible at a snapshot.
pub trait DataSource: IndexAccess + Send + Sync {
    /// Latest visible commit (default snapshot of a query).
    fn latest_committed_ts(&self) -> CommitTs;

    /// The row `row_id` as of `snapshot_ts`.
    fn point_lookup(
        &self,
        row_id: RowId,
        snapshot_ts: CommitTs,
    ) -> Result<Option<Row>, ExecutionError>;

    /// Up to `limit` rows of `range` visible at `snapshot_ts`, in key order, with keys strictly
    /// after `after`. An empty result means the range is exhausted.
    ///
    /// Scans are paged with this keyset cursor so memory stays bounded by the batch size.
    fn scan_page(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
        snapshot_ts: CommitTs,
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
    /// visible at `snapshot_ts`, in `direction`, after the position `resume` of the previous
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
        snapshot_ts: CommitTs,
    ) -> Result<IndexPage, ExecutionError> {
        let _ = (index, bounds, direction, fields, resume, limit, snapshot_ts);
        Err(ExecutionError::DataSource(
            "this data source has no indexes".into(),
        ))
    }

    /// The rows `row_ids` (in ascending order) as of `snapshot_ts`, in the same order; `None`
    /// for a row that is not visible. Implementations visit each storage page once.
    fn fetch_rows(
        &self,
        row_ids: &[RowId],
        snapshot_ts: CommitTs,
    ) -> Result<Vec<Option<Row>>, ExecutionError> {
        let _ = (row_ids, snapshot_ts);
        Err(ExecutionError::DataSource(
            "this data source cannot fetch rows in batches".into(),
        ))
    }
}
