//! File-backed implementation of the fixed-page persistence abstraction.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use adb_core::PageId;
use adb_page::{PAGE_SIZE, Page, PageKind};
use parking_lot::Mutex;

use crate::{BufferError, PageStore};

/// Persists fixed-size database pages in a single random-access file.
pub struct FilePageStore {
    file: Mutex<File>,
}

/// Implements opening and crash-tail normalization for the page file.
impl FilePageStore {
    /// Opens or creates the page file and truncates an incomplete trailing page left by a crash.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BufferError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;
        let len = file.metadata()?.len();
        let remainder = len % PAGE_SIZE as u64;
        if remainder != 0 {
            file.set_len(len - remainder)?;
            file.sync_data()?;
        }
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

/// Implements random-access fixed-page persistence.
impl PageStore for FilePageStore {
    /// Returns the number of complete pages in the normalized backing file.
    fn page_count(&self) -> Result<u64, BufferError> {
        Ok(self.file.lock().metadata()?.len() / PAGE_SIZE as u64)
    }

    /// Allocates and durably appends a new initialized page.
    fn allocate_page(&self, kind: PageKind) -> Result<Page, BufferError> {
        let mut file = self.file.lock();
        let len = file.metadata()?.len();
        if len % PAGE_SIZE as u64 != 0 {
            return Err(BufferError::CorruptStore(
                "page file is not page aligned".into(),
            ));
        }
        let id = PageId(len / PAGE_SIZE as u64);
        let page = Page::new(id, kind);
        let mut sealed = page.clone();
        sealed.seal_for_write();
        file.seek(SeekFrom::End(0))?;
        file.write_all(sealed.bytes())?;
        Ok(page)
    }

    /// Reads and validates one complete page.
    fn read_page(&self, page_id: PageId) -> Result<Page, BufferError> {
        let mut file = self.file.lock();
        let offset = page_id
            .0
            .checked_mul(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page offset overflow".into()))?;
        let len = file.metadata()?.len();
        let end = offset
            .checked_add(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page range overflow".into()))?;
        if end > len {
            return Err(BufferError::MissingPage(page_id.0));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0u8; PAGE_SIZE];
        file.read_exact(&mut bytes)?;
        Ok(Page::from_bytes(page_id, bytes)?)
    }

    /// Writes a complete page after sealing it with the hardened page checksum.
    fn write_page(&self, page: &Page) -> Result<(), BufferError> {
        let mut file = self.file.lock();
        let mut sealed = page.clone();
        sealed.seal_for_write();
        let offset = page
            .id
            .0
            .checked_mul(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page offset overflow".into()))?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(sealed.bytes())?;
        Ok(())
    }

    /// Synchronizes page data with stable storage.
    fn sync(&self) -> Result<(), BufferError> {
        self.file.lock().sync_data()?;
        Ok(())
    }
}
