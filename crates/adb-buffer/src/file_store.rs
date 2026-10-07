//! Single-file page store with crash-tail normalization.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use adb_core::PageId;
use adb_page::{Page, PAGE_SIZE};
use parking_lot::Mutex;

use crate::{BufferError, PageStore};

/// Byte offset of `id` inside a page file.
pub fn page_offset(id: PageId) -> Result<u64, BufferError> {
    id.0.checked_mul(PAGE_SIZE as u64)
        .ok_or_else(|| BufferError::CorruptStore("page offset overflow".into()))
}

/// Persists fixed-size pages in one random-access file.
pub struct FilePageStore {
    /// Path of the page file.
    path: PathBuf,
    /// Open file handle; the mutex serializes seek+read/write pairs.
    file: Mutex<File>,
}

impl FilePageStore {
    /// Opens or creates the page file and drops an incomplete trailing page left by a crash.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BufferError> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let len = file.metadata()?.len();
        let remainder = len % PAGE_SIZE as u64;
        if remainder != 0 {
            file.set_len(len - remainder)?;
            file.sync_data()?;
        }
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Path of the backing file; used to address journal writes.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl PageStore for FilePageStore {
    /// Complete pages in the file.
    fn page_count(&self) -> Result<u64, BufferError> {
        Ok(self.file.lock().metadata()?.len() / PAGE_SIZE as u64)
    }

    /// Reads page `id` and validates its header and checksum.
    fn read_page(&self, id: PageId) -> Result<Page, BufferError> {
        let mut file = self.file.lock();
        let offset = page_offset(id)?;
        let end = offset
            .checked_add(PAGE_SIZE as u64)
            .ok_or_else(|| BufferError::CorruptStore("page end overflow".into()))?;
        if end > file.metadata()?.len() {
            return Err(BufferError::MissingPage(id.0));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0u8; PAGE_SIZE];
        file.read_exact(&mut bytes)?;
        Ok(Page::from_bytes(id, bytes)?)
    }

    /// Seals the checksum and writes the page at its offset (extending the file if needed).
    fn write_page(&self, page: &Page) -> Result<(), BufferError> {
        let mut copy = page.clone();
        copy.seal_checksum();
        let mut file = self.file.lock();
        file.seek(SeekFrom::Start(page_offset(copy.id)?))?;
        file.write_all(copy.bytes())?;
        Ok(())
    }

    /// `fdatasync`s the page file.
    fn sync(&self) -> Result<(), BufferError> {
        self.file.lock().sync_data()?;
        Ok(())
    }
}
