//! Table and column statistics for the cost-based optimizer (Milestone 2.2.3).
//!
//! `ANALYZE` reads one entity at one snapshot through the execution layer's [`DataSource`]
//! and summarizes every column: exact row, NULL and min/max figures, an NDV estimate (exact
//! up to a threshold, HyperLogLog above it), and an equi-depth histogram plus most common
//! values built from a reservoir sample.
//!
//! Statistics are *derived data*: they are never written to the log, can be deleted at any
//! time and are rebuilt by the next `ANALYZE`. The engine persists them next to the data;
//! the JVM planner reads them as JSON ([`TableStatistics::to_json`]).
//!
//! [`DataSource`]: adb_execution::DataSource
pub mod error;
pub mod model;
pub mod options;
pub mod values;

pub use error::StatsError;
pub use model::{
    ColumnStatistics, HistogramBucket, MostCommonValue, TableStatistics, MAX_STATISTICS_JSON_BYTES,
    STATISTICS_FORMAT_VERSION,
};
pub use options::AnalyzeOptions;
