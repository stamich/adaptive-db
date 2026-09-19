//! Module `page_store` for crate `adb-buffer`.
use adb_core::PageId;
use adb_page::{Page, PageKind};

use crate::BufferError;

/// Defines the `PageStore` behavior contract for this subsystem.
pub trait PageStore: Send + Sync {
    /// Implements the `page_count` operation used by this subsystem.
    fn page_count(&self) -> Result<u64, BufferError>;
    /// Implements the `allocate_page` operation used by this subsystem.
    fn allocate_page(&self, kind: PageKind) -> Result<Page, BufferError>;
    /// Implements the `read_page` operation used by this subsystem.
    fn read_page(&self, page_id: PageId) -> Result<Page, BufferError>;
    /// Implements the `write_page` operation used by this subsystem.
    fn write_page(&self, page: &Page) -> Result<(), BufferError>;
    /// Implements the `sync` operation used by this subsystem.
    fn sync(&self) -> Result<(), BufferError>;
}
