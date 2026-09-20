//! Module `mod` for crate `src`.
pub mod filter;
pub mod limit;
pub mod point_lookup;
pub mod project;
pub mod scan;

use adb_core::{Row, RowId};

use crate::{ExecutionContext, ExecutionError};

/// Defines the `RowBatch` type alias used by this subsystem.
pub type RowBatch = Vec<(RowId, Row)>;

/// Defines the `Operator` behavior contract for this subsystem.
pub trait Operator: Send {
    /// Implements the `next_batch` operation used by this subsystem.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError>;
}
