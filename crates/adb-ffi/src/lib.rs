//! Module `lib` for crate `adb-ffi`.
pub mod batch;
pub mod database;
pub mod error;
pub mod handle;
pub mod query;
pub mod status;
pub mod version;

pub use batch::{adb_batch_data, adb_batch_len, adb_batch_release};
pub use database::{adb_close, adb_open};
pub use error::{adb_last_error_len, adb_last_error_ptr};
pub use query::{adb_execute_plan_json, adb_query_cancel, adb_query_close, adb_query_next_batch};
pub use status::AdbStatus;
pub use version::{adb_abi_version, adb_engine_version};
