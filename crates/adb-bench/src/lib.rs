//! The benchmark harness of Adaptive DB (Milestone 2.5.4.1).
//!
//! Before 2.5.4.1 the benchmarks of this repository reported one mean or one rate per case and
//! recorded almost nothing about the machine. The same engine code wrote 5.1k rows/s in one run
//! and 42.0k rows/s in the next because of the CPU governor, and nothing in the reports said so.
//! This crate is the shared answer, modelled on the harness of the ACE project (`ace-bench`):
//!
//! | module | responsibility |
//! |---|---|
//! | [`context`] | [`Context`]: where the data directory lives, the environment, the probe |
//! | [`plan`] | [`MeasurementPlan`]: warm-up, calibration, batches and samples per batch |
//! | [`runner`] | [`measure`]: the measurement loop; returns raw per-invocation samples |
//! | [`stats`] | [`SampleStats`]: median of medians, batch MAD, outliers, stability verdict |
//! | [`environment`] | [`Environment`]: governor, frequency, temperature, load, toolchain |
//! | [`storage`] | [`StorageInfo`]: filesystem, mount and device of the data directory |
//! | [`probe`] | [`FsyncProbe`]: a fixed `write` + `fdatasync` used as a control |
//! | [`report`] | the schema-3 JSON documents, which keep every key earlier reports had |
//!
//! The crate has no dependency on the engine; it only times closures.
#![forbid(unsafe_code)]

pub mod context;
pub mod environment;
pub mod plan;
pub mod probe;
pub mod report;
pub mod runner;
pub mod stats;
pub mod storage;

pub use context::Context;
pub use environment::{Environment, GovernorClass, RuntimeSnapshot};
pub use plan::MeasurementPlan;
pub use probe::FsyncProbe;
pub use report::Report;
pub use runner::{measure, measure_with_setup, Measured};
pub use stats::{SampleStats, Stability};
pub use storage::{MountEntry, StorageInfo};
