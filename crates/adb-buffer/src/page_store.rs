//! Page Store module for the adb-buffer crate.
//!
use adb_core::PageId;
use adb_page::{Page, PageKind};

use crate::BufferError;

/// Defines the `PageStore` abstraction implemented by pluggable components of this subsystem.
pub trait PageStore: Send + Sync {
    /// Returns the number of complete fixed-size pages currently present in the backing store.
    fn page_count(&self) -> Result<u64, BufferError>;
    /// Allocates and initializes a new fixed-size database page.
    fn allocate_page(&self, kind: PageKind) -> Result<Page, BufferError>;
    /// Reads and validates one page from persistent page storage.
    fn read_page(&self, page_id: PageId) -> Result<Page, BufferError>;
    /// Writes one complete database page at its fixed file offset.
    fn write_page(&self, page: &Page) -> Result<(), BufferError>;
    /// Flushes buffered WAL bytes and asks the operating system to synchronize file data.
    fn sync(&self) -> Result<(), BufferError>;
}
