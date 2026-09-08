//! Lib module for the adb-engine crate.
//!
pub mod database;
pub mod error;
pub mod recovery;

pub use database::Database;
pub use error::DbError;
