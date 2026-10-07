//! Pull-based physical execution over a snapshot-consistent [`DataSource`].
pub mod batch;
pub mod cancellation;
pub mod context;
pub mod datasource;
pub mod error;
pub mod executor;
pub mod expression;
pub mod metrics;
pub mod operator;
pub mod physical_plan;
pub mod wire;

pub use batch::{ColumnVector, PhysicalType, RecordBatch};
pub use cancellation::CancellationToken;
pub use context::{ExecutionContext, DEFAULT_BATCH_SIZE};
pub use datasource::DataSource;
pub use error::ExecutionError;
pub use executor::{Executor, QueryCursor};
pub use expression::{BinaryOp, Expr};
pub use metrics::QueryMetrics;
pub use physical_plan::PhysicalPlan;
pub use wire::{encode_batch_v1, BATCH_FORMAT_VERSION, BATCH_MAGIC, MAX_BATCH_WIRE_BYTES};
