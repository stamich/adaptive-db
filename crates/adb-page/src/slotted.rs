//! Slotted module for the adb-page crate.
//!
use adb_core::SlotId;

use crate::{PAGE_HEADER_SIZE, PAGE_SIZE, Page, PageError};

/// Defines the `SLOT_SIZE` constant used by this subsystem.
const SLOT_SIZE: usize = 4;

/// Provides slotted-page tuple insertion and lookup over a mutable heap page.
pub struct SlottedPage<'a> {
    page: &'a mut Page,
}

/// Implements behavior for `SlottedPage<'a>`.
impl<'a> SlottedPage<'a> {
    /// Creates a new instance initialized with the supplied state.
    pub fn new(page: &'a mut Page) -> Self {
        Self { page }
    }

    /// Inserts a new item into the underlying page, heap, tree, or transaction-local mutation set.
    pub fn insert(&mut self, bytes: &[u8]) -> Result<SlotId, PageError> {
        if bytes.len() > u16::MAX as usize {
            return Err(PageError::PayloadTooLarge(bytes.len()));
        }
        let free_start = self.page.free_start() as usize;
        let free_end = self.page.free_end() as usize;
        let needed = SLOT_SIZE + bytes.len();
        if free_end < free_start || free_end - free_start < needed {
            return Err(PageError::Full);
        }

        let slot_id = self.page.slot_count();
        let tuple_start = free_end - bytes.len();
        self.page.bytes_mut()[tuple_start..free_end].copy_from_slice(bytes);

        let slot_off = PAGE_HEADER_SIZE
            .checked_add(slot_id as usize * SLOT_SIZE)
            .ok_or_else(|| PageError::Corrupt("slot directory offset overflow".into()))?;
        let slot_end = slot_off
            .checked_add(SLOT_SIZE)
            .ok_or_else(|| PageError::Corrupt("slot directory range overflow".into()))?;
        if slot_end > tuple_start {
            return Err(PageError::Full);
        }
        self.page.bytes_mut()[slot_off..slot_off + 2]
            .copy_from_slice(&(tuple_start as u16).to_le_bytes());
        self.page.bytes_mut()[slot_off + 2..slot_off + 4]
            .copy_from_slice(&(bytes.len() as u16).to_le_bytes());

        self.page.set_slot_count(slot_id + 1);
        self.page.set_free_start((free_start + SLOT_SIZE) as u16);
        self.page.set_free_end(tuple_start as u16);
        Ok(slot_id)
    }

    /// Returns the value visible for the requested key or row at the operation's default snapshot.
    pub fn get(&self, slot_id: SlotId) -> Result<&[u8], PageError> {
        if slot_id >= self.page.slot_count() {
            return Err(PageError::InvalidSlot(slot_id));
        }
        let slot_off = PAGE_HEADER_SIZE
            .checked_add(slot_id as usize * SLOT_SIZE)
            .ok_or_else(|| PageError::Corrupt("slot offset overflow".into()))?;
        let first = self
            .page
            .bytes()
            .get(slot_off..slot_off + 2)
            .ok_or_else(|| PageError::Corrupt("slot offset outside page".into()))?;
        let second = self
            .page
            .bytes()
            .get(slot_off + 2..slot_off + 4)
            .ok_or_else(|| PageError::Corrupt("slot length outside page".into()))?;
        let offset = u16::from_le_bytes([first[0], first[1]]) as usize;
        let len = u16::from_le_bytes([second[0], second[1]]) as usize;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| PageError::Corrupt("slot range overflow".into()))?;
        if offset < self.page.free_end() as usize || end > PAGE_SIZE {
            return Err(PageError::Corrupt("slot points outside tuple area".into()));
        }
        self.page
            .bytes()
            .get(offset..end)
            .ok_or_else(|| PageError::Corrupt("slot slice outside page".into()))
    }
}
