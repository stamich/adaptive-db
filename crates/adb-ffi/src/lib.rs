//! Native C ABI facade for Adaptive DB Milestone 2.0.1 hardened.

pub mod batch;
pub mod database;
pub mod error;
pub mod handle;
pub mod metadata;
pub mod mutation;
pub mod query;
pub mod status;
pub mod version;

pub use batch::{adb_batch_data, adb_batch_len, adb_batch_release};
pub use database::{adb_close, adb_open};
pub use error::{adb_last_error_len, adb_last_error_ptr};
pub use metadata::adb_latest_committed_ts;
pub use mutation::{adb_delete_row, adb_insert_row_json, adb_update_fields_json};
pub use query::{
    adb_execute_plan_json, adb_execute_plan_json_at, adb_query_cancel, adb_query_close,
    adb_query_next_batch,
};
pub use status::AdbStatus;
pub use version::{adb_abi_version, adb_engine_version};
