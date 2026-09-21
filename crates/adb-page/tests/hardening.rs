//! Regression tests for hardened page validation.

use adb_core::PageId;
use adb_page::{Page, PageKind};

/// Verifies a hardened persisted page cannot bypass integrity checking by storing checksum zero.
#[test]
fn hardened_zero_checksum_is_rejected() {
    let page = Page::new(PageId(1), PageKind::Heap);
    let bytes = *page.bytes();
    assert!(Page::from_bytes(PageId(1), bytes).is_err());
}

/// Verifies structurally invalid heap free-space bounds are rejected even with a valid checksum.
#[test]
fn invalid_heap_bounds_are_rejected() {
    let mut page = Page::new(PageId(2), PageKind::Heap);
    page.set_free_start(100);
    page.set_free_end(50);
    page.seal_checksum();
    assert!(Page::from_bytes(PageId(2), *page.bytes()).is_err());
}
