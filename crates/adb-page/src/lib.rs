//! Module `lib` for crate `adb-page`.
pub mod error;
pub mod page;
pub mod slotted;

pub use error::PageError;
pub use page::{Page, PageKind, PAGE_FORMAT_VERSION, PAGE_HEADER_SIZE, PAGE_MAGIC, PAGE_SIZE};
pub use slotted::SlottedPage;
