//! Transactions: optimistic concurrency control over MVCC snapshots.
//!
//! A transaction reads a consistent snapshot, buffers its writes locally and is validated at
//! commit (backward OCC):
//!
//! * [`IsolationLevel::Snapshot`] — first-committer-wins on the write set. Write skew is possible.
//! * [`IsolationLevel::Serializable`] (default) — additionally every row the transaction *read*
//!   must be unchanged since its snapshot. A successful commit is then equivalent to executing
//!   the whole transaction atomically at its commit point, which rules out write skew.
//!
//! This crate is storage-agnostic: validation receives a lookup of the latest commit timestamp
//! per row (dependency inversion).
pub mod isolation;
pub mod manager;
pub mod mutation;
pub mod snapshot;
pub mod transaction;
pub mod validation;

pub use isolation::IsolationLevel;
pub use manager::TransactionManager;
pub use mutation::Mutation;
pub use snapshot::{SnapshotLease, SnapshotRegistry};
pub use transaction::Transaction;
pub use validation::{validate, Conflict};
