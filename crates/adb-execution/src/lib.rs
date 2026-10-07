//! Pull-based physical execution over a snapshot-consistent [`DataSource`].
//!
//! Operators exchange [`ExecRow`]s addressed by query-scoped [`SlotId`]s; storage field ids
//! appear only in the leaf nodes that map them to slots.
pub mod batch;
pub mod cancellation;
pub mod context;
pub mod datasource;
pub mod error;
pub mod exec_row;
pub mod executor;
pub mod expression;
pub mod limits;
pub mod memory;
pub mod metrics;
pub mod operator;
pub mod physical_plan;
pub mod slot;
pub mod validate;
pub mod wire;

pub use batch::{ColumnVector, PhysicalType, RecordBatch};
pub use cancellation::CancellationToken;
pub use context::{ExecutionContext, DEFAULT_BATCH_SIZE};
pub use datasource::DataSource;
pub use error::ExecutionError;
pub use exec_row::{ExecRow, RowBatch};
pub use executor::{Executor, QueryCursor};
pub use expression::{BinaryOp, Expr};
pub use limits::ExecutionLimits;
pub use memory::{MemoryReservation, MemoryTracker};
pub use metrics::QueryMetrics;
pub use physical_plan::PhysicalPlan;
pub use slot::{ScanColumn, SlotId};
pub use validate::PlanShape;
pub use wire::{
    encode_batch, BATCH_FORMAT_VERSION, BATCH_MAGIC, FLAG_ROW_IDS, MAX_BATCH_WIRE_BYTES,
};
