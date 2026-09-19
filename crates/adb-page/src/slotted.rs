//! Slotted-page tuple insertion and validated lookup.
use crate::{Page, PageError, PAGE_HEADER_SIZE, PAGE_SIZE};
use adb_core::SlotId;
/// Number of bytes in one `(offset,len)` slot-directory entry.
const SLOT_SIZE: usize = 4;
/// Mutable view over a heap page using a compact slot directory.
pub struct SlottedPage<'a> {
    page: &'a mut Page,
}
/// Implements bounded tuple insertion and slot lookup.
impl<'a> SlottedPage<'a> {
    /// Wraps a page for slotted-page operations.
    pub fn new(page: &'a mut Page) -> Self {
        Self { page }
    }
    /// Inserts a tuple when both tuple bytes and a new slot entry fit.
    pub fn insert(&mut self, bytes: &[u8]) -> Result<SlotId, PageError> {
        if bytes.len() > u16::MAX as usize {
            return Err(PageError::PayloadTooLarge(bytes.len()));
        }
        let free_start = self.page.free_start() as usize;
        let free_end = self.page.free_end() as usize;
        let needed = SLOT_SIZE
            .checked_add(bytes.len())
            .ok_or_else(|| PageError::PayloadTooLarge(bytes.len()))?;
        if free_end < free_start || free_end - free_start < needed {
            return Err(PageError::Full);
        }
        let slot_id = self.page.slot_count();
        if slot_id == u16::MAX {
            return Err(PageError::Full);
        }
        let slot_off = PAGE_HEADER_SIZE
            .checked_add(
                (slot_id as usize)
                    .checked_mul(SLOT_SIZE)
                    .ok_or_else(|| PageError::Corrupt("slot offset overflow".into()))?,
            )
            .ok_or_else(|| PageError::Corrupt("slot offset overflow".into()))?;
        if slot_off + SLOT_SIZE > free_end {
            return Err(PageError::Full);
        }
        let tuple_start = free_end - bytes.len();
        self.page.bytes_mut()[tuple_start..free_end].copy_from_slice(bytes);
        self.page.bytes_mut()[slot_off..slot_off + 2]
            .copy_from_slice(&(tuple_start as u16).to_le_bytes());
        self.page.bytes_mut()[slot_off + 2..slot_off + 4]
            .copy_from_slice(&(bytes.len() as u16).to_le_bytes());
        self.page.set_slot_count(slot_id + 1);
        self.page.set_free_start((free_start + SLOT_SIZE) as u16);
        self.page.set_free_end(tuple_start as u16);
        Ok(slot_id)
    }
    /// Returns tuple bytes after validating slot-directory and tuple-area bounds.
    pub fn get(&self, slot_id: SlotId) -> Result<&[u8], PageError> {
        if slot_id >= self.page.slot_count() {
            return Err(PageError::InvalidSlot(slot_id));
        }
        let slot_off = PAGE_HEADER_SIZE
            .checked_add(
                (slot_id as usize)
                    .checked_mul(SLOT_SIZE)
                    .ok_or_else(|| PageError::Corrupt("slot offset overflow".into()))?,
            )
            .ok_or_else(|| PageError::Corrupt("slot offset overflow".into()))?;
        let slot = self
            .page
            .bytes()
            .get(slot_off..slot_off + SLOT_SIZE)
            .ok_or_else(|| PageError::Corrupt("slot directory outside page".into()))?;
        let offset = u16::from_le_bytes([slot[0], slot[1]]) as usize;
        let len = u16::from_le_bytes([slot[2], slot[3]]) as usize;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| PageError::Corrupt("tuple range overflow".into()))?;
        if offset < (self.page.free_end() as usize) || end > PAGE_SIZE {
            return Err(PageError::Corrupt("slot points outside tuple area".into()));
        }
        Ok(&self.page.bytes()[offset..end])
    }
}
