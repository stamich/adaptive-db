//! Module `mutation` for crate `adb-tx`.
use adb_core::Row;

/// Enumerates `Mutation` alternatives used by this subsystem.
#[derive(Debug, Clone)]
pub enum Mutation {
    Put(Row),
    Delete,
}
