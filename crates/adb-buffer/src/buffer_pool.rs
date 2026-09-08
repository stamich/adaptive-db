//! Buffer Pool module for the adb-buffer crate.
//!
use std::{collections::HashMap, sync::Arc};

use adb_core::PageId;
use adb_page::{Page, PageKind};
use parking_lot::Mutex;

use crate::{BufferError, PageStore};

/// Represents `Frame` state used by the src subsystem.
struct Frame {
    page: Page,
    dirty: bool,
    last_used: u64,
}

/// Represents `State` state used by the src subsystem.
struct State {
    frames: HashMap<PageId, Frame>,
    clock: u64,
}

/// Caches database pages, tracks dirty state, and evicts unpinned frames when capacity is reached.
pub struct BufferPool {
    store: Arc<dyn PageStore>,
    capacity: usize,
    state: Mutex<State>,
}

/// Implements behavior for `BufferPool`.
impl BufferPool {
    /// Creates a new instance initialized with the supplied state.
    pub fn new(store: Arc<dyn PageStore>, capacity: usize) -> Self {
        // A zero-page pool is unusable and previously panicked. Harden it to the smallest
        // meaningful capacity so configuration errors cannot terminate the process.
        let capacity = capacity.max(1);
        Self {
            store,
            capacity,
            state: Mutex::new(State {
                frames: HashMap::new(),
                clock: 0,
            }),
        }
    }

    /// Returns the number of complete fixed-size pages currently present in the backing store.
    pub fn page_count(&self) -> Result<u64, BufferError> {
        self.store.page_count()
    }

    /// Allocates and initializes a new fixed-size database page.
    pub fn allocate_page(&self, kind: PageKind) -> Result<PageId, BufferError> {
        let page = self.store.allocate_page(kind)?;
        let id = page.id;
        let mut state = self.state.lock();
        self.ensure_capacity_locked(&mut state)?;
        state.clock += 1;
        let used = state.clock;
        state.frames.insert(
            id,
            Frame {
                page,
                dirty: false,
                last_used: used,
            },
        );
        Ok(id)
    }

    /// Implements the `read` operation used by this subsystem.
    pub fn read<R>(&self, page_id: PageId, f: impl FnOnce(&Page) -> R) -> Result<R, BufferError> {
        let mut state = self.state.lock();
        self.load_locked(&mut state, page_id)?;
        state.clock += 1;
        let used = state.clock;
        let frame = state
            .frames
            .get_mut(&page_id)
            .ok_or(BufferError::MissingPage(page_id.0))?;
        // The state mutex remains held while the closure runs, so this frame cannot be
        // evicted concurrently. Avoid manual pin increments whose decrement could be skipped
        // if caller code panics.
        frame.last_used = used;
        Ok(f(&frame.page))
    }

    /// Implements the `write` operation used by this subsystem.
    pub fn write<R>(
        &self,
        page_id: PageId,
        f: impl FnOnce(&mut Page) -> R,
    ) -> Result<R, BufferError> {
        let mut state = self.state.lock();
        self.load_locked(&mut state, page_id)?;
        state.clock += 1;
        let used = state.clock;
        let frame = state
            .frames
            .get_mut(&page_id)
            .ok_or(BufferError::MissingPage(page_id.0))?;
        // The state mutex itself pins the frame for the closure's lifetime.
        frame.last_used = used;
        let result = f(&mut frame.page);
        frame.dirty = true;
        Ok(result)
    }

    /// Writes all dirty cached pages and synchronizes the backing page store.
    pub fn flush_all(&self) -> Result<(), BufferError> {
        let mut state = self.state.lock();
        for frame in state.frames.values_mut() {
            if frame.dirty {
                self.store.write_page(&frame.page)?;
                frame.dirty = false;
            }
        }
        self.store.sync()
    }

    /// Implements the `load_locked` operation used by this subsystem.
    fn load_locked(&self, state: &mut State, page_id: PageId) -> Result<(), BufferError> {
        if state.frames.contains_key(&page_id) {
            return Ok(());
        }
        self.ensure_capacity_locked(state)?;
        let page = self.store.read_page(page_id)?;
        state.clock += 1;
        let used = state.clock;
        state.frames.insert(
            page_id,
            Frame {
                page,
                dirty: false,
                last_used: used,
            },
        );
        Ok(())
    }

    /// Implements the `ensure_capacity_locked` operation used by this subsystem.
    fn ensure_capacity_locked(&self, state: &mut State) -> Result<(), BufferError> {
        if state.frames.len() < self.capacity {
            return Ok(());
        }
        let victim = state
            .frames
            .iter()
            .min_by_key(|(_, frame)| frame.last_used)
            .map(|(id, _)| *id)
            .ok_or(BufferError::NoEvictableFrame)?;
        let frame = state
            .frames
            .remove(&victim)
            .ok_or(BufferError::NoEvictableFrame)?;
        if frame.dirty {
            self.store.write_page(&frame.page)?;
        }
        Ok(())
    }
}
