//! Module `handle` for crate `adb-ffi`.
use adb_engine::Database;
use adb_execution::QueryCursor;
use parking_lot::Mutex;

/// Represents `AdbDatabaseHandle` state used by this subsystem.
pub struct AdbDatabaseHandle {
    pub(crate) database: Database,
}

/// Represents `AdbQueryHandle` state used by this subsystem.
pub struct AdbQueryHandle {
    pub(crate) cursor: Mutex<QueryCursor>,
}

/// Represents `AdbBatchHandle` state used by this subsystem.
pub struct AdbBatchHandle {
    pub(crate) bytes: Box<[u8]>,
}
