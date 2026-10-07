//! Heap file: variable-length tuples in slotted pages.
//!
//! With [`SpaceReuse::Reclaim`] the heap keeps an in-memory free-space map (rebuilt from the
//! pages on open), deletes free their slot, and inserts go to any page with enough room.
//! With [`SpaceReuse::AppendOnly`] (immutable history) tuples are only ever appended.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_buffer::{BufferPool, FilePageStore};
use adb_core::{Lsn, PageId, RowLocation};
use adb_journal::FileWrite;
use adb_page::{PageError, PageKind, SlottedPage, SlottedView};
use parking_lot::Mutex;

use crate::{Checkpointable, StorageError};

/// Pages with less free space than this are not worth tracking.
const MIN_TRACKED_FREE: usize = 64;

/// Space-management policy of a heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceReuse {
    /// Tuples are never deleted; inserts append.
    AppendOnly,
    /// Deleted tuples free space that later inserts reuse.
    Reclaim,
}

/// Free space per page, searchable by size.
#[derive(Default)]
struct FreeSpaceMap {
    /// `(free bytes, page)` ordered for smallest-fit search.
    by_size: BTreeSet<(usize, PageId)>,
    /// Free bytes per tracked page (to update `by_size`).
    by_page: HashMap<PageId, usize>,
}

impl FreeSpaceMap {
    /// Records `free` bytes for `page`; pages below the tracking threshold are dropped.
    fn set(&mut self, page: PageId, free: usize) {
        if let Some(old) = self.by_page.remove(&page) {
            self.by_size.remove(&(old, page));
        }
        if free >= MIN_TRACKED_FREE {
            self.by_page.insert(page, free);
            self.by_size.insert((free, page));
        }
    }

    /// Smallest-fit page with at least `needed` bytes (keeps big holes for big tuples).
    fn find(&self, needed: usize) -> Option<PageId> {
        self.by_size
            .range((needed, PageId(0))..)
            .next()
            .map(|(_, page)| *page)
    }
}

/// Slotted-page heap backed by one page file.
pub struct HeapFile {
    /// Page cache of the heap file.
    pool: Arc<BufferPool>,
    /// Path of the heap file (target of journal writes).
    path: PathBuf,
    /// Whether deleted space is reused.
    policy: SpaceReuse,
    /// Free space per page (maintained only with `Reclaim`).
    free: Mutex<FreeSpaceMap>,
}

impl HeapFile {
    /// Opens or creates a heap file.
    pub fn open(
        path: impl AsRef<Path>,
        buffer_pages: usize,
        policy: SpaceReuse,
    ) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        let store = Arc::new(FilePageStore::open(&path)?);
        let heap = Self {
            pool: Arc::new(BufferPool::new(store, buffer_pages)?),
            path,
            policy,
            free: Mutex::new(FreeSpaceMap::default()),
        };
        if policy == SpaceReuse::Reclaim {
            let mut free = heap.free.lock();
            for id in 0..heap.pool.page_count() {
                let page = PageId(id);
                free.set(page, heap.available(page)?);
            }
        }
        Ok(heap)
    }

    /// Stores a tuple and returns its location.
    pub fn insert(&self, bytes: &[u8], lsn: Lsn) -> Result<RowLocation, StorageError> {
        let mut free = self.free.lock();
        let candidate = match self.policy {
            SpaceReuse::Reclaim => free.find(bytes.len()),
            SpaceReuse::AppendOnly => self.pool.page_count().checked_sub(1).map(PageId),
        };
        if let Some(page_id) = candidate {
            if let Some(location) = self.try_insert(&mut free, page_id, bytes, lsn)? {
                return Ok(location);
            }
        }
        let page_id = self.pool.allocate_page(PageKind::Heap)?;
        self.try_insert(&mut free, page_id, bytes, lsn)?
            .ok_or(StorageError::Page(PageError::PayloadTooLarge(bytes.len())))
    }

    /// Frees the tuple at `location`.
    pub fn delete(&self, location: RowLocation, lsn: Lsn) -> Result<(), StorageError> {
        if self.policy == SpaceReuse::AppendOnly {
            return Err(StorageError::Invalid(
                "delete on an append-only heap".into(),
            ));
        }
        let mut free = self.free.lock();
        let available = self.pool.write(location.page_id, |page| {
            page.set_page_lsn(lsn);
            let mut slotted = SlottedPage::new(page);
            slotted.delete(location.slot_id)?;
            slotted.available_for_insert()
        })??;
        free.set(location.page_id, available);
        Ok(())
    }

    /// Reads the tuple at `location`.
    pub fn read(&self, location: RowLocation) -> Result<Vec<u8>, StorageError> {
        Ok(self.pool.read(location.page_id, |page| {
            SlottedView::new(page)
                .get(location.slot_id)
                .map(<[u8]>::to_vec)
        })??)
    }

    /// Logical page count.
    pub fn page_count(&self) -> u64 {
        self.pool.page_count()
    }

    /// Persists all pages directly (not crash-atomic; standalone use only).
    pub fn flush(&self) -> Result<(), StorageError> {
        Ok(self.pool.flush_all()?)
    }

    /// Inserts into `page_id` if it fits and updates the free-space map; `None` when full.
    fn try_insert(
        &self,
        free: &mut FreeSpaceMap,
        page_id: PageId,
        bytes: &[u8],
        lsn: Lsn,
    ) -> Result<Option<RowLocation>, StorageError> {
        let (result, available) = self.pool.write(page_id, |page| {
            let mut slotted = SlottedPage::new(page);
            let result = slotted.insert(bytes);
            let available = slotted.available_for_insert();
            if result.is_ok() {
                page.set_page_lsn(lsn);
            }
            (result, available)
        })?;
        if self.policy == SpaceReuse::Reclaim {
            free.set(page_id, available?);
        }
        match result {
            Ok(slot_id) => Ok(Some(RowLocation { page_id, slot_id })),
            Err(PageError::Full) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Largest tuple `page_id` can hold after compaction.
    fn available(&self, page_id: PageId) -> Result<usize, StorageError> {
        Ok(self.pool.read(page_id, |page| {
            SlottedView::new(page).available_for_insert()
        })??)
    }
}

impl Checkpointable for HeapFile {
    /// Dirty heap pages as journal writes.
    fn journal_writes(&self) -> Result<Vec<FileWrite>, StorageError> {
        Ok(self.pool.journal_writes(&self.path)?)
    }

    /// Marks all heap pages persisted.
    fn mark_clean(&self) {
        self.pool.mark_clean();
    }

    /// Dirty heap pages.
    fn dirty_pages(&self) -> usize {
        self.pool.dirty_count()
    }
}
