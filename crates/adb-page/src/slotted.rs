//! Slotted heap page: variable-length tuples addressed by stable slot ids.
//!
//! Layout (after the common page header):
//!
//! ```text
//! [slot 0][slot 1]...[slot n-1]  ->free space<-  [tuple k]...[tuple 0]
//! ^ free_start grows right                       ^ free_end grows left
//! ```
//!
//! Each slot is `(offset: u16, len: u16)`. A slot with `offset == 0` is **free**: offset 0 is
//! inside the page header, so it can never be a real tuple address. Freed slots are reused by
//! later inserts, and freed tuple bytes are reclaimed by compaction, so slot ids stay stable for
//! live tuples while the page never leaks space.

use adb_core::SlotId;

use crate::{Page, PageError, PAGE_HEADER_SIZE, PAGE_SIZE};

/// Bytes of one `(offset, len)` slot-directory entry.
const SLOT_SIZE: usize = 4;
/// Offset value marking a free slot.
const FREE_SLOT: u16 = 0;

/// Read-only view over a heap page (no page copy needed for lookups).
pub struct SlottedView<'a> {
    /// The page being read.
    page: &'a Page,
}

impl<'a> SlottedView<'a> {
    /// Wraps a heap page for reading.
    pub fn new(page: &'a Page) -> Self {
        Self { page }
    }

    /// Returns the bytes of a live tuple.
    pub fn get(&self, slot_id: SlotId) -> Result<&'a [u8], PageError> {
        if slot_id >= self.page.slot_count() {
            return Err(PageError::InvalidSlot(slot_id));
        }
        let (offset, len) = read_slot(self.page, slot_id);
        if offset == FREE_SLOT {
            return Err(PageError::InvalidSlot(slot_id));
        }
        let (offset, len) = (offset as usize, len as usize);
        if offset < self.page.free_end() as usize || offset + len > PAGE_SIZE {
            return Err(PageError::Corrupt("slot points outside tuple area".into()));
        }
        Ok(&self.page.bytes()[offset..offset + len])
    }

    /// Largest tuple an insert can store right now (after compaction).
    pub fn available_for_insert(&self) -> Result<usize, PageError> {
        let directory = self.page.slot_count() as usize * SLOT_SIZE;
        let mut live = 0usize;
        let mut has_free_slot = false;
        for slot in 0..self.page.slot_count() {
            let (offset, len) = read_slot(self.page, slot);
            if offset == FREE_SLOT {
                has_free_slot = true;
            } else {
                live += len as usize;
            }
        }
        let free = (PAGE_SIZE - PAGE_HEADER_SIZE)
            .checked_sub(directory + live)
            .ok_or_else(|| PageError::Corrupt("live tuples exceed page".into()))?;
        Ok(if has_free_slot {
            free
        } else {
            free.saturating_sub(SLOT_SIZE)
        })
    }

    /// Number of live tuples.
    pub fn live_count(&self) -> usize {
        (0..self.page.slot_count())
            .filter(|slot| read_slot(self.page, *slot).0 != FREE_SLOT)
            .count()
    }
}

/// Raw `(offset, len)` of a slot-directory entry.
fn read_slot(page: &Page, slot_id: SlotId) -> (u16, u16) {
    let at = PAGE_HEADER_SIZE + slot_id as usize * SLOT_SIZE;
    let bytes = page.bytes();
    (
        u16::from_le_bytes([bytes[at], bytes[at + 1]]),
        u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]),
    )
}

/// Mutable view over a heap page.
pub struct SlottedPage<'a> {
    /// The page being modified.
    page: &'a mut Page,
}

impl<'a> SlottedPage<'a> {
    /// Wraps a heap page.
    pub fn new(page: &'a mut Page) -> Self {
        Self { page }
    }

    /// Stores `bytes` and returns its slot. Reuses free slots and compacts when needed.
    pub fn insert(&mut self, bytes: &[u8]) -> Result<SlotId, PageError> {
        if bytes.len() > PAGE_SIZE - PAGE_HEADER_SIZE - SLOT_SIZE {
            return Err(PageError::PayloadTooLarge(bytes.len()));
        }
        if self.available_for_insert()? < bytes.len() {
            return Err(PageError::Full);
        }

        let reused = self.first_free_slot();
        let directory_growth = if reused.is_some() { 0 } else { SLOT_SIZE };
        if self.contiguous_free() < bytes.len() + directory_growth {
            self.compact()?;
        }

        let slot_id = match reused {
            Some(slot) => slot,
            None => {
                let slot = self.page.slot_count();
                if slot == u16::MAX {
                    return Err(PageError::Full);
                }
                self.page.set_slot_count(slot + 1);
                self.page
                    .set_free_start(self.page.free_start() + SLOT_SIZE as u16);
                slot
            }
        };

        let tuple_start = self.page.free_end() as usize - bytes.len();
        self.page.bytes_mut()[tuple_start..tuple_start + bytes.len()].copy_from_slice(bytes);
        self.page.set_free_end(tuple_start as u16);
        self.write_slot(slot_id, tuple_start as u16, bytes.len() as u16);
        Ok(slot_id)
    }

    /// Returns the bytes of a live tuple.
    pub fn get(&self, slot_id: SlotId) -> Result<&[u8], PageError> {
        SlottedView::new(self.page).get(slot_id)
    }

    /// Frees a live tuple. Its slot id may be reused by a later insert; its bytes are reclaimed
    /// lazily by compaction. Trailing free slots are trimmed from the directory immediately.
    pub fn delete(&mut self, slot_id: SlotId) -> Result<(), PageError> {
        self.live_slot(slot_id)?;
        self.write_slot(slot_id, FREE_SLOT, 0);
        while let Some(last) = self.page.slot_count().checked_sub(1) {
            if self.read_slot(last).0 != FREE_SLOT {
                break;
            }
            self.page.set_slot_count(last);
            self.page
                .set_free_start(self.page.free_start() - SLOT_SIZE as u16);
        }
        Ok(())
    }

    /// Largest tuple that [`insert`](Self::insert) can store right now (after compaction).
    pub fn available_for_insert(&self) -> Result<usize, PageError> {
        SlottedView::new(self.page).available_for_insert()
    }

    /// Number of live tuples.
    pub fn live_count(&self) -> usize {
        SlottedView::new(self.page).live_count()
    }

    /// Moves all live tuples to the end of the page, removing holes left by deletes.
    fn compact(&mut self) -> Result<(), PageError> {
        let mut live = Vec::new();
        for slot in 0..self.page.slot_count() {
            if self.read_slot(slot).0 != FREE_SLOT {
                live.push((slot, self.get(slot)?.to_vec()));
            }
        }
        let mut cursor = PAGE_SIZE;
        for (slot, bytes) in live {
            cursor -= bytes.len();
            self.page.bytes_mut()[cursor..cursor + bytes.len()].copy_from_slice(&bytes);
            self.write_slot(slot, cursor as u16, bytes.len() as u16);
        }
        self.page.set_free_end(cursor as u16);
        Ok(())
    }

    /// Bytes between the slot directory and the tuple area.
    fn contiguous_free(&self) -> usize {
        (self.page.free_end() as usize).saturating_sub(self.page.free_start() as usize)
    }

    /// Lowest free slot id, if any.
    fn first_free_slot(&self) -> Option<SlotId> {
        (0..self.page.slot_count()).find(|slot| self.read_slot(*slot).0 == FREE_SLOT)
    }

    /// `(offset, len)` of a live slot, or `InvalidSlot`.
    fn live_slot(&self, slot_id: SlotId) -> Result<(usize, usize), PageError> {
        if slot_id >= self.page.slot_count() {
            return Err(PageError::InvalidSlot(slot_id));
        }
        let (offset, len) = self.read_slot(slot_id);
        if offset == FREE_SLOT {
            return Err(PageError::InvalidSlot(slot_id));
        }
        Ok((offset as usize, len as usize))
    }

    /// Raw `(offset, len)` of a slot-directory entry.
    fn read_slot(&self, slot_id: SlotId) -> (u16, u16) {
        read_slot(self.page, slot_id)
    }

    /// Overwrites a slot-directory entry.
    fn write_slot(&mut self, slot_id: SlotId, offset: u16, len: u16) {
        let at = PAGE_HEADER_SIZE + slot_id as usize * SLOT_SIZE;
        let bytes = self.page.bytes_mut();
        bytes[at..at + 2].copy_from_slice(&offset.to_le_bytes());
        bytes[at + 2..at + 4].copy_from_slice(&len.to_le_bytes());
    }
}
