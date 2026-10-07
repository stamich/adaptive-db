//! No-steal page cache.
//!
//! Invariants:
//! * a dirty page stays in memory until it is checkpointed ([`BufferPool::journal_writes`] +
//!   [`BufferPool::mark_clean`]) or flushed ([`BufferPool::flush_all`]);
//! * newly allocated pages exist only in memory until then, so a crash can never leave a
//!   half-initialized or orphaned page behind in the file;
//! * only clean pages are evicted. When every cached page is dirty the pool grows past its
//!   nominal capacity; the engine bounds that growth by checkpointing on a dirty-page budget.

use std::{collections::HashMap, path::Path, sync::Arc};

use adb_core::PageId;
use adb_journal::FileWrite;
use adb_page::{Page, PageKind};
use parking_lot::Mutex;

use crate::{page_offset, BufferError, PageStore};

/// One cached page.
struct Frame {
    /// The page image.
    page: Page,
    /// Whether the image differs from the persisted page.
    dirty: bool,
    /// Logical clock value of the last access (LRU eviction of clean frames).
    last_used: u64,
}

/// Mutable pool state guarded by one mutex.
struct State {
    /// Cached frames by page id.
    frames: HashMap<PageId, Frame>,
    /// Logical clock advanced on every access.
    clock: u64,
    /// Logical number of pages, including allocated pages not yet written to the store.
    page_count: u64,
    /// Number of dirty frames (kept incrementally; read on every commit).
    dirty: usize,
}

/// Page cache over one [`PageStore`].
pub struct BufferPool {
    /// Persistent location of the pages.
    store: Arc<dyn PageStore>,
    /// Nominal number of cached pages (exceeded only by dirty pages).
    capacity: usize,
    /// Frames, clock, page count and dirty counter.
    state: Mutex<State>,
}

impl BufferPool {
    /// Creates a pool that nominally caches `capacity` pages.
    pub fn new(store: Arc<dyn PageStore>, capacity: usize) -> Result<Self, BufferError> {
        let page_count = store.page_count()?;
        Ok(Self {
            store,
            capacity: capacity.max(1),
            state: Mutex::new(State {
                frames: HashMap::new(),
                clock: 0,
                page_count,
                dirty: 0,
            }),
        })
    }

    /// Logical page count (persisted pages plus pages allocated since the last checkpoint).
    pub fn page_count(&self) -> u64 {
        self.state.lock().page_count
    }

    /// Allocates a fresh in-memory page; it reaches disk with the next checkpoint.
    pub fn allocate_page(&self, kind: PageKind) -> Result<PageId, BufferError> {
        let mut state = self.state.lock();
        self.evict_if_full(&mut state);
        let id = PageId(state.page_count);
        state.page_count += 1;
        state.dirty += 1;
        state.clock += 1;
        let clock = state.clock;
        state.frames.insert(
            id,
            Frame {
                page: Page::new(id, kind),
                dirty: true,
                last_used: clock,
            },
        );
        Ok(id)
    }

    /// Runs `f` over a read-only view of a page.
    pub fn read<R>(&self, page_id: PageId, f: impl FnOnce(&Page) -> R) -> Result<R, BufferError> {
        let mut state = self.state.lock();
        let frame = self.frame(&mut state, page_id)?;
        Ok(f(&frame.page))
    }

    /// Runs `f` over a mutable view of a page and marks it dirty.
    pub fn write<R>(
        &self,
        page_id: PageId,
        f: impl FnOnce(&mut Page) -> R,
    ) -> Result<R, BufferError> {
        let mut state = self.state.lock();
        let frame = self.frame(&mut state, page_id)?;
        let newly_dirty = !frame.dirty;
        frame.dirty = true;
        let result = f(&mut frame.page);
        if newly_dirty {
            state.dirty += 1;
        }
        Ok(result)
    }

    /// Number of pages that differ from their persisted image.
    pub fn dirty_count(&self) -> usize {
        self.state.lock().dirty
    }

    /// Sealed images of every dirty page as journal writes against `path`, in page order.
    ///
    /// Call [`mark_clean`](Self::mark_clean) once the journal holding them has been committed;
    /// the caller must prevent concurrent writes in between.
    pub fn journal_writes(&self, path: &Path) -> Result<Vec<FileWrite>, BufferError> {
        let state = self.state.lock();
        let mut dirty: Vec<&Frame> = state.frames.values().filter(|frame| frame.dirty).collect();
        dirty.sort_by_key(|frame| frame.page.id);
        dirty
            .into_iter()
            .map(|frame| {
                let mut sealed = frame.page.clone();
                sealed.seal_checksum();
                Ok(FileWrite::range(
                    path,
                    page_offset(sealed.id)?,
                    sealed.bytes().to_vec(),
                ))
            })
            .collect()
    }

    /// Declares every cached page identical to its persisted image.
    pub fn mark_clean(&self) {
        let mut state = self.state.lock();
        for frame in state.frames.values_mut() {
            frame.dirty = false;
        }
        state.dirty = 0;
    }

    /// Writes every dirty page in place and syncs the store.
    ///
    /// This is **not** crash-atomic across pages; the engine uses journaled checkpoints instead.
    /// It exists for initialization of empty structures and for standalone tools/tests.
    pub fn flush_all(&self) -> Result<(), BufferError> {
        let mut state = self.state.lock();
        let mut dirty: Vec<&mut Frame> = state
            .frames
            .values_mut()
            .filter(|frame| frame.dirty)
            .collect();
        dirty.sort_by_key(|frame| frame.page.id);
        for frame in dirty {
            self.store.write_page(&frame.page)?;
            frame.dirty = false;
        }
        state.dirty = 0;
        self.store.sync()
    }

    /// Returns the cached frame for `page_id`, loading it from the store if needed.
    fn frame<'s>(
        &self,
        state: &'s mut State,
        page_id: PageId,
    ) -> Result<&'s mut Frame, BufferError> {
        if page_id.0 >= state.page_count {
            return Err(BufferError::MissingPage(page_id.0));
        }
        if !state.frames.contains_key(&page_id) {
            self.evict_if_full(state);
            let page = self.store.read_page(page_id)?;
            state.frames.insert(
                page_id,
                Frame {
                    page,
                    dirty: false,
                    last_used: 0,
                },
            );
        }
        state.clock += 1;
        let clock = state.clock;
        let frame = state
            .frames
            .get_mut(&page_id)
            .ok_or_else(|| BufferError::CorruptStore("frame vanished".into()))?;
        frame.last_used = clock;
        Ok(frame)
    }

    /// Evicts the least recently used **clean** frame when the pool is at capacity.
    fn evict_if_full(&self, state: &mut State) {
        if state.frames.len() < self.capacity {
            return;
        }
        let victim = state
            .frames
            .iter()
            .filter(|(_, frame)| !frame.dirty)
            .min_by_key(|(_, frame)| frame.last_used)
            .map(|(id, _)| *id);
        if let Some(victim) = victim {
            state.frames.remove(&victim);
        }
    }
}
