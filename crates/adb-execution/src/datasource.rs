//! Module `datasource` for crate `adb-execution`.
use adb_core::{CommitTs, Row, RowId};

use crate::ExecutionError;

/// Narrow execution-facing contract. The execution crate intentionally knows
/// nothing about B+Tree pages, heap slots, WAL, or transaction internals.
pub trait DataSource: Send + Sync {
    /// Implements the `latest_committed_ts` operation used by this subsystem.
    fn latest_committed_ts(&self) -> CommitTs;

    /// Implements the `point_lookup` operation used by this subsystem.
    fn point_lookup(
        &self,
        row_id: RowId,
        snapshot_ts: CommitTs,
    ) -> Result<Option<Row>, ExecutionError>;

    /// Milestone 1.7 captures a stable set of logical rows before the operator
    /// starts producing batches. This favors correctness over lock-free leaf
    /// traversal while the B+Tree still uses coarse-grained mutation locking.
    fn scan_rows(&self, snapshot_ts: CommitTs) -> Result<Vec<(RowId, Row)>, ExecutionError>;
}
