//! Module `heap` for crate `adb-storage`.
use std::{path::Path, sync::Arc};

use adb_buffer::{BufferPool, FilePageStore};
use adb_core::{Lsn, PageId, RowLocation};
use adb_page::{PageError, PageKind, SlottedPage};

use crate::StorageError;

/// Represents `HeapFile` state used by this subsystem.
pub struct HeapFile {
    pool: Arc<BufferPool>,
}

/// Implements behavior for `HeapFile`.
impl HeapFile {
    /// Implements the `open` operation used by this subsystem.
    pub fn open(path: impl AsRef<Path>, buffer_pages: usize) -> Result<Self, StorageError> {
        let store = Arc::new(FilePageStore::open(path)?);
        Ok(Self {
            pool: Arc::new(BufferPool::new(store, buffer_pages)),
        })
    }

    /// Implements the `insert` operation used by this subsystem.
    pub fn insert(&self, bytes: &[u8], lsn: Lsn) -> Result<RowLocation, StorageError> {
        let count = self.pool.page_count()?;
        if count > 0 {
            let page_id = PageId(count - 1);
            let result = self.pool.write(page_id, |page| {
                page.set_page_lsn(lsn);
                SlottedPage::new(page).insert(bytes)
            })?;
            match result {
                Ok(slot_id) => return Ok(RowLocation { page_id, slot_id }),
                Err(PageError::Full) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let page_id = self.pool.allocate_page(PageKind::Heap)?;
        let slot_id = self.pool.write(page_id, |page| {
            page.set_page_lsn(lsn);
            SlottedPage::new(page).insert(bytes)
        })??;
        Ok(RowLocation { page_id, slot_id })
    }

    /// Implements the `read` operation used by this subsystem.
    pub fn read(&self, location: RowLocation) -> Result<Vec<u8>, StorageError> {
        let bytes = self.pool.read(location.page_id, |page| {
            let mut clone = page.clone();
            let slotted = SlottedPage::new(&mut clone);
            slotted.get(location.slot_id).map(|v| v.to_vec())
        })??;
        Ok(bytes)
    }

    /// Implements the `flush` operation used by this subsystem.
    pub fn flush(&self) -> Result<(), StorageError> {
        self.pool.flush_all()?;
        Ok(())
    }

    /// Implements the `page_count` operation used by this subsystem.
    pub fn page_count(&self) -> Result<u64, StorageError> {
        Ok(self.pool.page_count()?)
    }
}
