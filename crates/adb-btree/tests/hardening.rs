//! Hardening tests for bounded B+Tree node decoding.

use adb_btree::codec::decode_node;
use adb_core::PageId;
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE};

/// Verifies a corrupt leaf count returns an error instead of panicking.
#[test]
fn corrupt_leaf_count_is_rejected() {
    let mut page = Page::new(PageId(1), PageKind::BTreeLeaf);
    page.bytes_mut()[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 2]
        .copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode_node(&page).is_err());
}
