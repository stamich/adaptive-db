//! What the executor needs from storage.
use adb_core::{CommitTs, KeyRange, Row, RowId};

use crate::ExecutionError;

/// Snapshot-consistent row access. The executor knows nothing about pages, heaps, logs or
/// transactions; it only asks for rows visible at a snapshot.
pub trait DataSource: Send + Sync {
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
