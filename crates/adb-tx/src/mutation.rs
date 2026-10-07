//! Buffered write of one row.
use adb_core::Row;

/// The pending change of one row inside a transaction.
#[derive(Debug, Clone, PartialEq)]
pub enum Mutation {
    /// Insert or replace with this content.
    Put(Row),
    /// Delete.
    Delete,
}
