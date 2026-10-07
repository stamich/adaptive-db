//! Isolation levels offered by the engine.

/// How strictly a transaction is validated at commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IsolationLevel {
    /// Snapshot isolation: write-write conflicts abort; write skew is possible.
    Snapshot,
    /// Serializable: write-write and read-write conflicts abort.
    #[default]
    Serializable,
}
