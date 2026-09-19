//! File-backed fixed-page persistence with crash-tail normalization.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use adb_core::PageId;
use adb_page::{Page, PageKind, PAGE_SIZE};
use parking_lot::Mutex;

use crate::{BufferError, PageStore};

/// Persists fixed-size pages in one random-access file.
pub struct FilePageStore {
    file: Mutex<File>,
}

/// Implements page-file opening and incomplete trailing-page normalization.
impl FilePageStore {
    /// Opens or creates the page file and truncates only an incomplete final physical page.
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

    /// Computes a checked byte offset for a page id.
    fn offset(id: PageId) -> Result<u64, BufferError> {
        id.0.checked_mul(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page offset overflow".into()))
    }
}

/// Implements random-access page persistence.
impl PageStore for FilePageStore {
    /// Returns the number of complete fixed-size pages in the normalized file.
    fn page_count(&self) -> Result<u64, BufferError> {
        Ok(self.file.lock().metadata()?.len() / PAGE_SIZE as u64)
    }

    /// Allocates and appends one initialized checksummed page.
    fn allocate_page(&self, kind: PageKind) -> Result<Page, BufferError> {
        let mut file = self.file.lock();
        let len = file.metadata()?.len();
        if len % PAGE_SIZE as u64 != 0 {
            return Err(BufferError::CorruptStore("page file is not aligned".into()));
        }
        let id = PageId(len / PAGE_SIZE as u64);
        let page = Page::new(id, kind);
        let mut sealed = page.clone();
        sealed.seal_checksum();
        file.seek(SeekFrom::End(0))?;
        file.write_all(sealed.bytes())?;
        Ok(page)
    }

    /// Reads exactly one existing page and validates its common header and checksum.
    fn read_page(&self, id: PageId) -> Result<Page, BufferError> {
        let mut file = self.file.lock();
        let offset = Self::offset(id)?;
        let len = file.metadata()?.len();
        let end = offset
            .checked_add(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page end overflow".into()))?;
        if end > len {
            return Err(BufferError::MissingPage(id.0));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0u8; PAGE_SIZE];
        file.read_exact(&mut bytes)?;
        Ok(Page::from_bytes(id, bytes)?)
    }

    /// Writes one existing page after upgrading/sealing it with the hardened checksum format.
    fn write_page(&self, page: &Page) -> Result<(), BufferError> {
        let mut copy = page.clone();
        copy.seal_checksum();
        let mut file = self.file.lock();
        let offset = Self::offset(copy.id)?;
        let end = offset
            .checked_add(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page end overflow".into()))?;
        if end > file.metadata()?.len() {
            return Err(BufferError::MissingPage(copy.id.0));
        }
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(copy.bytes())?;
        Ok(())
    }

    /// Synchronizes page data to stable storage.
    fn sync(&self) -> Result<(), BufferError> {
        self.file.lock().sync_data()?;
        Ok(())
    }
}
