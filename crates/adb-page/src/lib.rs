//! Lib module for the adb-page crate.
//!
pub mod error;
pub mod page;
pub mod slotted;

pub use error::PageError;
pub use page::{PAGE_FORMAT_VERSION, PAGE_HEADER_SIZE, PAGE_MAGIC, PAGE_SIZE, Page, PageKind};
pub use slotted::SlottedPage;
