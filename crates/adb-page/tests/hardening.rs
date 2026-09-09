//! Hardening tests for page checksum and structural validation.

use adb_core::PageId;
use adb_page::{PAGE_SIZE, Page, PageKind};

/// Verifies checksum corruption is detected before higher-level decoding.
#[test]
fn checksum_corruption_is_rejected() {
    let mut page = Page::new(PageId(1), PageKind::Heap);
    page.seal_for_write();
    let mut bytes = *page.bytes();
    bytes[100] ^= 0x55;
    assert!(Page::from_bytes(PageId(1), bytes).is_err());
}

/// Verifies invalid free-space metadata is rejected.
#[test]
fn invalid_free_space_bounds_are_rejected() {
    let mut page = Page::new(PageId(2), PageKind::Heap);
    page.bytes_mut()[8..10].copy_from_slice(&(PAGE_SIZE as u16).to_le_bytes());
    page.bytes_mut()[10..12].copy_from_slice(&(32u16).to_le_bytes());
    page.seal_for_write();
    let bytes = *page.bytes();
    assert!(Page::from_bytes(PageId(2), bytes).is_err());
}
