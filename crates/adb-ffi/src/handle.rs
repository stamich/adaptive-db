//! Opaque handle types owned by the library and lent to C callers.
use adb_engine::Database;
use adb_execution::QueryCursor;
use parking_lot::Mutex;

/// An open database (`adb_open` / `adb_close`).
pub struct AdbDatabaseHandle {
    /// The open database.
    pub(crate) database: Database,
}

/// A running query cursor (`adb_execute_plan_json*` / `adb_query_close`).
pub struct AdbQueryHandle {
    /// The running query; the mutex makes the handle safe to use from several threads.
    pub(crate) cursor: Mutex<QueryCursor>,
}

/// An immutable byte buffer: an encoded record batch or a JSON document (`adb_batch_release`).
pub struct AdbBatchHandle {
    /// Buffer contents.
    pub(crate) bytes: Box<[u8]>,
}

/// A cursor over the change log (`adb_change_cursor_open` / `adb_change_cursor_close`).
pub struct AdbChangeCursorHandle {
    /// The cursor; the mutex makes the handle safe to use from several threads.
    pub(crate) cursor: Mutex<adb_change_log::ChangeCursor>,
}
