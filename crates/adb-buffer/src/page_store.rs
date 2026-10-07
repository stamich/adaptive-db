//! Abstraction over the persistent location of fixed-size pages.
use adb_core::PageId;
use adb_page::Page;

use crate::BufferError;

/// Random-access persistence of fixed-size pages. Allocation is the buffer pool's job.
pub trait PageStore: Send + Sync {
    /// Number of complete pages currently persisted.
    fn page_count(&self) -> Result<u64, BufferError>;
    /// Reads and validates one persisted page.
    fn read_page(&self, page_id: PageId) -> Result<Page, BufferError>;
    /// Seals and writes one page, extending the store when the page lies past its end.
    fn write_page(&self, page: &Page) -> Result<(), BufferError>;
    /// Makes all previous writes durable.
    fn sync(&self) -> Result<(), BufferError>;
}
