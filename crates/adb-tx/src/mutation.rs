//! Mutation module for the adb-tx crate.
//!
use adb_core::Row;

/// Enumerates the supported `Mutation` variants used by this subsystem.
#[derive(Debug, Clone)]
pub enum Mutation {
    Put(Row),
    Delete,
}
