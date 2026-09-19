//! Module `lib` for crate `adb-engine`.
pub mod database;
pub mod error;
pub mod recovery;

pub use database::Database;
pub use error::DbError;
