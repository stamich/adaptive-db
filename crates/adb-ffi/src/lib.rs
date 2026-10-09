//! C ABI of Adaptive DB (ABI version 5, Milestone 2.2.3). See `include/adb.h`.
//!
//! Every entry point validates its raw arguments, initializes its output slots first, contains
//! panics, and reports failures as an [`AdbStatus`] plus a thread-local error message.
//!
//! The entry points are safe `extern "C"` functions that dereference caller-supplied pointers
//! after validating them; their pointer contracts are documented in `include/adb.h`. Marking
//! them `unsafe` would add nothing for C callers, hence the lint exemption below.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod args;
pub mod batch;
pub mod cdc;
pub mod database;
pub mod error;
pub mod handle;
pub mod maintenance;
pub mod metadata;
pub mod mutation;
pub mod query;
pub mod statistics;
pub mod status;
pub mod version;

pub use batch::{adb_batch_data, adb_batch_len, adb_batch_release};
pub use cdc::{
    adb_change_feed_end, adb_commit_consumer_offset, adb_consumer_offset, adb_read_changes_json,
};
pub use database::{adb_close, adb_open};
pub use error::{adb_last_error_len, adb_last_error_ptr};
pub use maintenance::{adb_checkpoint, adb_vacuum};
pub use metadata::adb_latest_committed_ts;
pub use mutation::{adb_delete_row, adb_insert_row_json, adb_update_fields_json};
pub use query::{
    adb_execute_plan_json, adb_execute_plan_json_at, adb_query_cancel, adb_query_close,
    adb_query_next_batch, adb_query_profile_json,
};
pub use statistics::{
    adb_analyze_entity_json, adb_modifications_since_analyze, adb_statistics_generation,
    adb_statistics_json,
};
pub use status::AdbStatus;
pub use version::{adb_abi_version, adb_engine_version};
