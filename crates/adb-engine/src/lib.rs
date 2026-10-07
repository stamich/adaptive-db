//! Adaptive DB storage engine (data plane).
//!
//! * [`Database`] — transactions, snapshot reads, physical-plan execution, change feed,
//!   checkpoints and vacuum.
//! * The canonical log ([`adb_wal`]) is the source of truth; the current and version stores
//!   are rebuildable projections of it.
//! * [`cdc`] — native change-data capture read directly from the log.
pub mod cdc;
pub mod committed;
pub mod database;
pub mod error;
pub mod health;
pub mod log;
pub mod offsets;
pub mod options;
pub mod projections;
mod recovery;

pub use adb_tx::{Conflict, IsolationLevel, Transaction};
pub use cdc::{ChangeBatch, ChangeCursor, ChangeEvent, ChangeFilter, ChangeKind, RowChange};
pub use database::{Database, VacuumReport};
pub use error::DbError;
pub use options::DatabaseOptions;
